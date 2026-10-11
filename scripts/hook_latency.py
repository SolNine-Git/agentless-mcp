"""Time the erosion hook on one mcp-local Write in each cache state, per frozen build.

Development-time only, with the standard library alone. The method is the
overview's "Baselines" section in docs/plans/2026-10-10_rust-port-00-overview.md.

    python3 -I scripts/hook_latency.py --mcp-local ../mcp-local --build 1dc4d70 --build HEAD
"""

import argparse
import contextlib
import dataclasses
import hashlib
import io
import json
import math
import os
import platform
import shutil
import signal
import sqlite3
import statistics
import subprocess
import sys
import tarfile
import tempfile
import time
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
HOOK = Path("contrib/hooks/agentless_erosion_gate.py")
MCP_LOCAL_COMMIT = "f17af96"
TARGET = "src/readers/document_readers.py"
APPENDED = (
    "\n\ndef _probe_edit(value):\n    if value is None:\n        return 0\n    return len(value)\n"
)
GRAMMAR_CACHE = "tree-sitter-language-pack"
GRAMMAR_OVERRIDE = "TREE_SITTER_LANGUAGE_PACK_CACHE_DIR"
TAG_CACHE = "agentless-mcp"
# Every tenth source file goes stale after the index, so the stale files spread
# across the whole tree.
STALE_STRIDE = 10
RUNS = 30
P95 = 0.95

# Deadlines in seconds, above each call's measured time and inner bound, so that
# only a hung call reaches one.
LOCAL_DEADLINE_S = 30.0
SYNC_DEADLINE_S = 120.0
INDEX_DEADLINE_S = 60.0
# Claude Code's timeout for this hook in contrib/hooks/settings-example.json.
HOOK_DEADLINE_S = 15.0

GATE_OFF = "gate off"
ABSENT = "no tag cache"
FRESH = "fresh tag cache"
STALE = "stale tag cache"


@dataclass(frozen=True)
class Build:
    """One commit of this repository, archived and synced into its own directory."""

    rev: str
    root: Path

    @property
    def binary(self) -> Path:
        """Return the build's agentless-mcp executable."""
        return self.root / ".venv" / "bin" / "agentless-mcp"

    def cache(self, state: str) -> Path:
        """Return the XDG_CACHE_HOME this build uses in `state`."""
        kind = "indexed" if state in (FRESH, STALE) else "empty"
        return self.root.parent / f"xdg-{self.root.name}-{kind}"


@dataclass(frozen=True)
class Arm:
    """One build's hook command in one cache state."""

    build: Build
    command: list[str]
    env: dict[str, str]
    log: Path


@dataclass(frozen=True)
class Session:
    """What every timed run shares: the payload, the interpreter, the run count, the scratch dir."""

    payload: bytes
    python: str
    runs: int
    work: Path


@dataclass(frozen=True)
class Row:
    """The timed runs of one build in one cache state, and the outcomes its log recorded."""

    build: str
    state: str
    samples_ms: list[float]
    outcomes: dict[str, int]


def main(argv: list[str] | None = None) -> int:
    """Measure every build in every cache state, print the table, and write the samples."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--mcp-local", type=Path, required=True, help="a local mcp-local clone")
    parser.add_argument("--build", action="append", required=True, help="a commit of this repo")
    parser.add_argument("--runs", type=int, default=RUNS)
    parser.add_argument("--work", type=Path, help="an empty or absent scratch directory")
    args = parser.parse_args(argv)

    work = args.work or Path(tempfile.mkdtemp(prefix="hook-latency-"))
    work.mkdir(parents=True, exist_ok=True)
    python = shutil.which("python3")
    if any(work.iterdir()) or python is None:
        print(f"refused: {work} must be empty and python3 must be on PATH", file=sys.stderr)
        return 2
    grammars = _grammar_cache()
    clone = _clone_mcp_local(args.mcp_local, work)
    builds = [_freeze(rev, work, grammars) for rev in args.build]
    session = Session(_payload(clone, work), python, args.runs, work)

    rows = _measure(GATE_OFF, builds, session) + _measure(ABSENT, builds, session)
    _index_clone(builds, clone)
    rows += _measure(FRESH, builds, session)
    stale = _make_stale(clone)
    rows += _measure(STALE, builds, session)
    environment = _environment(python)
    samples = [dataclasses.asdict(row) for row in rows]
    report = {"environment": environment, "stale": stale, "rows": samples}
    out = work / "hook_latency.json"
    out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    _print_table(environment, stale, rows)
    print(f"samples: {out}")
    return 0


def _run(
    command: list[str],
    *,
    timeout: float,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    payload: bytes | None = None,
) -> subprocess.CompletedProcess[bytes]:
    stdin = subprocess.PIPE if payload is not None else None
    with subprocess.Popen(
        command,
        stdin=stdin,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=cwd,
        env=env,
        start_new_session=True,
    ) as process:
        try:
            stdout, stderr = process.communicate(payload, timeout=timeout)
        except subprocess.TimeoutExpired:
            message = (
                f"{' '.join(command)} ran past its {timeout:g} s deadline,"
                " and the processes it started were killed"
            )
            raise SystemExit(message) from None
        finally:
            # Also on Ctrl-C, which cannot reach a child in its own session.
            if process.returncode is None:
                _kill_tree(process.pid)
    if process.returncode != 0:
        detail = stderr.decode("utf-8", errors="replace").strip()
        message = f"{' '.join(command)} exited with {process.returncode}: {detail}"
        raise SystemExit(message)
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def _kill_tree(leader: int) -> None:
    # The leader is not reaped yet, so its group id names no other group. A process that leaves
    # its group and whose parent exits before this runs (a setsid daemon) escapes the walk.
    listing = subprocess.run(
        ["ps", "-A", "-o", "pid=", "-o", "ppid=", "-o", "pgid="],
        capture_output=True,
        check=True,
        timeout=LOCAL_DEADLINE_S,
    ).stdout.decode()
    pids, groups = _descendants(listing, leader)
    for pid in pids:
        with contextlib.suppress(ProcessLookupError):
            os.kill(pid, signal.SIGKILL)
    for group in groups:
        with contextlib.suppress(ProcessLookupError):
            os.killpg(group, signal.SIGKILL)


def _descendants(listing: str, leader: int) -> tuple[list[int], set[int]]:
    # A descendant can start its own process group, so the walk follows parents, not groups.
    children: dict[int, list[int]] = {}
    group: dict[int, int] = {}
    for line in listing.splitlines():
        pid, ppid, pgid = (int(field) for field in line.split())
        children.setdefault(ppid, []).append(pid)
        group[pid] = pgid
    tree = [leader]
    position = 0
    while position < len(tree):
        tree.extend(children.get(tree[position], []))
        position += 1
    return tree, {leader} | {group[pid] for pid in tree if pid in group}


def _grammar_cache() -> Path | None:
    override = os.environ.get(GRAMMAR_OVERRIDE)
    cache_home = Path(os.environ.get("XDG_CACHE_HOME") or Path.home() / ".cache")
    grammars = Path(override) if override else cache_home / GRAMMAR_CACHE
    if not grammars.is_dir():
        message = f"{grammars} is missing: run agentless-mcp index on any repository once"
        raise SystemExit(message)
    # Each build reads the override itself (_env keeps it), so only the XDG layout needs a link.
    return None if override else grammars


def _commit(repo: Path, rev: str) -> str:
    command = ["git", "-C", str(repo), "rev-parse", "--verify", "--end-of-options"]
    command.append(f"{rev}^{{commit}}")
    return _run(command, timeout=LOCAL_DEADLINE_S).stdout.decode().strip()


def _clone_mcp_local(source: Path, work: Path) -> Path:
    clone = work / "mcp-local"
    clone_command = ["git", "clone", "-q", "--no-hardlinks", str(source), str(clone)]
    _run(clone_command, timeout=LOCAL_DEADLINE_S)
    commit = _commit(clone, MCP_LOCAL_COMMIT)
    _run(["git", "-C", str(clone), "switch", "-q", "--detach", commit], timeout=LOCAL_DEADLINE_S)
    return clone


def _freeze(rev: str, work: Path, grammars: Path | None) -> Build:
    sha = _commit(REPO, rev)
    build = Build(rev=f"{rev} ({sha[:7]})", root=work / f"agentless-{sha[:7]}")
    archive = _run(["git", "-C", str(REPO), "archive", sha], timeout=LOCAL_DEADLINE_S).stdout
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(build.root, filter="data")
    _run(["uv", "sync", "--frozen", "-q"], cwd=build.root, timeout=SYNC_DEADLINE_S)
    for state in (ABSENT, FRESH):
        build.cache(state).mkdir()
        if grammars is not None:
            # An isolated cache must still hold the grammars, or the first run downloads them.
            (build.cache(state) / GRAMMAR_CACHE).symlink_to(grammars)
    return build


def _payload(clone: Path, work: Path) -> bytes:
    path = clone / TARGET
    content = path.read_text(encoding="utf-8") + APPENDED
    document = {
        "session_id": "s",
        "transcript_path": str(work / "none.jsonl"),
        "tool_use_id": "t",
        "tool_name": "Write",
        "hook_event_name": "PreToolUse",
        "cwd": str(clone),
        "tool_input": {"file_path": str(path), "content": content},
    }
    return json.dumps(document).encode("utf-8")


def _index_clone(builds: list[Build], clone: Path) -> None:
    for build in builds:
        command = [str(build.binary), "index", "--repo", str(clone)]
        _run(command, env=_env(build, FRESH, log=None), timeout=INDEX_DEADLINE_S)


def _make_stale(clone: Path) -> str:
    files = sorted((clone / "src").rglob("*.py"))
    changed = [path for path in files if path != clone / TARGET][::STALE_STRIDE]
    for path in changed:
        path.write_text(path.read_text(encoding="utf-8") + "\n# stale\n", encoding="utf-8")
    return f"{len(changed)} of {len(files)} files changed after index"


def _env(build: Build, state: str, *, log: Path | None) -> dict[str, str]:
    env = {key: value for key, value in os.environ.items() if not key.startswith("AGENTLESS_")}
    env["XDG_CACHE_HOME"] = str(build.cache(state))
    env["AGENTLESS_EROSION_BIN"] = str(build.binary)
    env["AGENTLESS_EROSION_GATE"] = "off" if state == GATE_OFF else "advisory"
    if log is not None:
        env["AGENTLESS_EROSION_LOG"] = str(log)
    return env


def _arm(build: Build, state: str, session: Session) -> Arm:
    log = session.work / f"log-{build.root.name}-{state.replace(' ', '-')}.jsonl"
    command = [session.python, str(build.root / HOOK)]
    return Arm(build=build, command=command, env=_env(build, state, log=log), log=log)


def _measure(state: str, builds: list[Build], session: Session) -> list[Row]:
    arms = [_arm(build, state, session) for build in builds]
    for arm in arms:
        _run(arm.command, env=arm.env, payload=session.payload, timeout=HOOK_DEADLINE_S)
    before = [_fingerprint(arm.build.cache(state)) for arm in arms]
    samples = _timed(arms, session.payload, session.runs)
    rows = []
    for arm, fingerprint, timings in zip(arms, before, samples, strict=True):
        if _fingerprint(arm.build.cache(state)) != fingerprint:
            message = f"{arm.build.rev}: the hook changed the tag cache in the {state} runs"
            raise SystemExit(message)
        rows.append(Row(arm.build.rev, state, timings, _outcomes(state, arm.log)))
    return rows


def _timed(arms: list[Arm], payload: bytes, runs: int) -> list[list[float]]:
    samples: list[list[float]] = [[] for _ in arms]
    for _ in range(runs):
        for arm, timings in zip(arms, samples, strict=True):
            start = time.perf_counter()
            _run(arm.command, env=arm.env, payload=payload, timeout=HOOK_DEADLINE_S)
            timings.append((time.perf_counter() - start) * 1000)
    return samples


def _fingerprint(cache: Path) -> str:
    # Rows, not file times: closing a WAL connection touches tags.db with no row changed.
    digest = hashlib.sha256()
    for database in sorted((cache / TAG_CACHE).glob("*/tags.db")):
        connection = sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True)
        try:
            for statement in sorted(connection.iterdump()):
                digest.update(statement.encode("utf-8"))
        finally:
            connection.close()
    return digest.hexdigest()


def _outcomes(state: str, log: Path) -> dict[str, int]:
    if state == GATE_OFF:
        return {}
    records = [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]
    expect_off = state == ABSENT
    for record in records:
        if ("clone_delta" in record["off_by_config"]) != expect_off:
            wanted = "off" if expect_off else "on"
            message = f"{log}: clone_delta is not {wanted} in the {state} state"
            raise SystemExit(message)
    return dict(Counter(record["outcome"] for record in records))


def _environment(python: str) -> dict[str, object]:
    return {
        "date": time.strftime("%Y-%m-%d"),
        "python3": _run([python, "--version"], timeout=LOCAL_DEADLINE_S).stdout.decode().strip(),
        "cpus": os.cpu_count(),
        "system": f"{platform.system()} {platform.release()}",
        "mcp_local": MCP_LOCAL_COMMIT,
        "payload": f"Write of {TARGET} with one 4-line function appended",
    }


def _percentile(samples: list[float], fraction: float) -> float:
    ordered = sorted(samples)
    return ordered[max(0, math.ceil(fraction * len(ordered)) - 1)]


def _print_table(environment: dict[str, object], stale: str, rows: list[Row]) -> None:
    print(json.dumps(environment))
    print(f"stale state: {stale}")
    print("| Build | State | Median ms | p95 ms | Min ms | Runs | Outcomes |")
    print("|---|---|---|---|---|---|---|")
    for row in rows:
        outcomes = ", ".join(f"{name} {count}" for name, count in sorted(row.outcomes.items()))
        cells = (
            row.build,
            row.state,
            f"{statistics.median(row.samples_ms):.0f}",
            f"{_percentile(row.samples_ms, P95):.0f}",
            f"{min(row.samples_ms):.0f}",
            str(len(row.samples_ms)),
            outcomes or "not logged",
        )
        print("| " + " | ".join(cells) + " |")


if __name__ == "__main__":
    raise SystemExit(main())
