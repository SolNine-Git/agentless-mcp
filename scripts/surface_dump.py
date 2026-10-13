"""Pick the surface-diff targets, or ask one agentless-mcp build every surface-diff query.

usage: surface_dump.py targets --harness DIR [--records FILE] [--limit N] --out FILE
       surface_dump.py dump --targets FILE --scratch DIR --out DIR [--workers N]
                            [--timeout SECONDS] -- ARGV_PREFIX...

Run both modes with the interpreter of the base build's frozen copy:

    uv run --project <frozen copy> python scripts/surface_dump.py ...

targets: samples symbols from the base build's full scan of each Loc-Bench checkout, so that
one target list serves both builds. Set HOME, XDG_CACHE_HOME and
TREE_SITTER_LANGUAGE_PACK_CACHE_DIR to a scratch directory that holds a copy of the grammar
libraries.

dump: runs ARGV_PREFIX, for example `uv run --project <frozen copy> agentless-mcp
--no-auto-warm` or the path of a binary, once per query as a subprocess with a timeout. Each
call gets an environment whose HOME, caches and temporary files are under --scratch. Copy the
grammar libraries to <scratch>/grammars first, and give each build its own scratch directory.
Each dump also records the find-symbol frame that its build lists, so that surface_cmp.py can
refuse two builds whose frames differ.
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from collections.abc import Iterator, Mapping, Sequence
from concurrent.futures import ThreadPoolExecutor
from dataclasses import asdict, dataclass
from functools import partial
from pathlib import Path
from typing import Any

from fixture_files import GRAMMAR_OVERRIDE, hermetic_env, refuse_real_caches
from surface_cmp import ANSWERS, BUILD, FRAME, TARGETS, Answer, Target, parse_targets, strip_receipt

from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import grammars, refs
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.symbols import symbol_stable_id

DEFAULT_RECORDS = Path("results") / "fix-base.jsonl"
DEFAULT_WORKERS = 4
DEFAULT_TIMEOUT_SECONDS = 60.0
KILL_GRACE_SECONDS = 10.0
SEEDS_PER_INSTANCE = 2
SAMPLED_SYMBOLS = 3
SAMPLED_KINDS = ("class", "function", "method")
REFS_LIMIT = "500"
FIND_LIMIT = "500"
NAME = re.compile(r"[A-Za-z_][A-Za-z_0-9]*")
PROJECT_FLAG = "--project"
SKIPPED_DIRS = frozenset({".git", ".venv", "target", "__pycache__"})
CACHE_DIR_SUFFIX = "_cache"


@dataclass(frozen=True)
class Record:
    """One Loc-Bench record: the instance, its repository and commit, and its seeds."""

    instance_id: str
    repo: str
    base_commit: str
    seeds: tuple[str, ...]


@dataclass(frozen=True)
class Build:
    """How to run one build: its argv prefix, environment, working directory and timeout."""

    prefix: tuple[str, ...]
    env: Mapping[str, str]
    cwd: Path
    timeout: float


def parse_record(line: str) -> Record:
    """Return the record that one JSONL line holds, or raise on a field of the wrong type."""
    raw = json.loads(line)
    words, seeds = (raw["instance_id"], raw["repo"], raw["base_commit"]), raw["seeds"]
    if not isinstance(seeds, list) or not all(isinstance(w, str) for w in (*words, *seeds)):
        message = f"malformed Loc-Bench record: {line[:200]}"
        raise TypeError(message)
    return Record(*words, tuple(seeds))


def checkout(harness: Path, record: Record) -> Path:
    """Return the directory that holds the record's checkout in the harness."""
    owner_repo = record.repo.replace("/", "__")
    return harness / "repos" / f"{owner_repo}__{record.base_commit}"


def pick_target(extractor: TreeSitterExtractor, harness: Path, record: Record) -> Target:
    """Return the record's queries, with symbols sampled from the full in-process scan."""
    root = checkout(harness, record)
    ctx = resolve_repo(root, None)
    scan = refs.scan_repo(ctx.root, extractor, source=ctx.symbols)
    ids = sorted(
        {
            symbol_stable_id(symbol)
            for facts in scan.files
            if refs.in_name_graph(facts)
            for symbol in facts.symbols
            if symbol.kind.value in SAMPLED_KINDS and not symbol.name.startswith("__")
        }
    )
    step = max(1, len(ids) // SAMPLED_SYMBOLS)
    names = [seed for seed in record.seeds if NAME.fullmatch(seed)]
    return Target(
        instance_id=record.instance_id,
        root=str(root),
        seeds=list(record.seeds),
        name_seeds=names[:SEEDS_PER_INSTANCE],
        sampled=ids[::step][:SAMPLED_SYMBOLS],
    )


def queries(target: Target) -> dict[str, list[str]]:
    """Return the subcommand argv of each query that one target receives, by answer key."""
    root = target.root
    focus = [part for seed in target.seeds for part in ("--focus", seed)]
    asked = {
        "map": ["map", "--repo", root, "--granularity", "function", *focus],
        "cycles": ["cycles", "--repo", root],
    }
    for name in target.name_seeds:
        asked[f"refs:{name}"] = ["refs", "--repo", root, "--json", "--limit", REFS_LIMIT, name]
        asked[f"explain:{name}"] = ["explain", "--repo", root, name]
    for sid in target.sampled:
        asked[f"refs:{sid}"] = ["refs", "--repo", root, "--json", "--limit", REFS_LIMIT, sid]
    return asked


def frame_queries(target: Target) -> dict[str, list[str]]:
    """Return the find-symbol listing argv of each sampled kind, by kind."""
    listing = ["--json", "--limit", FIND_LIMIT]
    return {
        kind: ["find-symbol", "--repo", target.root, "--kind", kind, *listing, ""]
        for kind in SAMPLED_KINDS
    }


def environment(scratch: Path) -> dict[str, str]:
    """Return the whole environment of a call: HOME, caches and temporary files in scratch."""
    return {
        **hermetic_env(scratch / "home"),
        "TMPDIR": str(scratch / "tmp"),
        GRAMMAR_OVERRIDE: str(scratch / "grammars"),
        "AGENTLESS_MCP_NO_AUTO_WARM": "1",
        "AGENTLESS_MCP_NO_DOWNLOAD": "1",
        # git status must not refresh the index of a Loc-Bench checkout.
        "GIT_OPTIONAL_LOCKS": "0",
        "LANG": "C.UTF-8",
    }


def run_call(build: Build, argv: Sequence[str]) -> tuple[Answer, float]:
    """Run one query, kill its process group at the timeout, and return its answer and seconds."""
    started = time.perf_counter()
    with subprocess.Popen(
        [*build.prefix, *argv],
        cwd=build.cwd,
        env=dict(build.env),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    ) as process:
        try:
            out, err = process.communicate(timeout=build.timeout)
            code: int | None = process.returncode
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            out, err = process.communicate(timeout=KILL_GRACE_SECONDS)
            code = None
    answer = Answer(code, _text(out), _text(err))
    return answer, round(time.perf_counter() - started, 2)


def _text(data: bytes) -> str:
    # surrogateescape keeps an undecodable byte distinct from any other in the comparison.
    return data.decode("utf-8", errors="surrogateescape")


def dump_target(build: Build, target: Target) -> dict[str, Any]:
    """Return every answer of one target, with argv and seconds, and its find-symbol frame."""
    answers = {}
    for key, argv in queries(target).items():
        answer, seconds = run_call(build, argv)
        answers[key] = {"argv": argv, "seconds": seconds, **asdict(answer)}
    frame = {}
    for kind, argv in frame_queries(target).items():
        answer, _ = run_call(build, argv)
        visible = strip_receipt(answer.stdout), strip_receipt(answer.stderr)
        frame[kind] = asdict(Answer(answer.code, *visible))
    return {"instance_id": target.instance_id, "answers": answers, "frame": frame}


def identity(prefix: Sequence[str]) -> dict[str, str]:
    """Return what the prefix runs: a project directory and its source digest, or a file's."""
    project = project_dir(prefix)
    if project is not None:
        return {"kind": "project", "path": str(project), "sha256": source_digest(project)}
    found = shutil.which(prefix[0])
    if found is None:
        sys.exit(f"refused: {prefix[0]} is not an executable on PATH")
    binary = Path(found).resolve()
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    return {"kind": "binary", "path": str(binary), "sha256": digest}


def project_dir(prefix: Sequence[str]) -> Path | None:
    """Return the directory that a `--project DIR` or `--project=DIR` word names in prefix."""
    for index, word in enumerate(prefix):
        if word == PROJECT_FLAG and index + 1 < len(prefix):
            return Path(prefix[index + 1]).resolve()
        if word.startswith(PROJECT_FLAG + "="):
            return Path(word.split("=", 1)[1]).resolve()
    return None


def source_digest(directory: Path) -> str:
    """Return the sha256 over the relative path and bytes of each source file in directory."""
    digest = hashlib.sha256()
    named = sorted((path.relative_to(directory).as_posix(), path) for path in _sources(directory))
    for name, path in named:
        data = str(path.readlink()).encode() if path.is_symlink() else path.read_bytes()
        digest.update(f"{name}\0{len(data)}\0".encode())
        digest.update(data)
    return digest.hexdigest()


def _sources(directory: Path) -> Iterator[Path]:
    for current, subdirs, files in os.walk(directory):
        subdirs[:] = [name for name in subdirs if not _skipped(name)]
        yield from (Path(current) / name for name in files)


def _skipped(name: str) -> bool:
    return name in SKIPPED_DIRS or (name.startswith(".") and name.endswith(CACHE_DIR_SUFFIX))


def warm_version(answer: Answer) -> str:
    """Return the version a `capabilities --json` answer reports, or refuse a cold grammar."""
    try:
        payload = json.loads(answer.stdout)
        cold = [entry["name"] for entry in payload["languages"] if entry["warmed"] is not True]
        version = payload["version"]
    except (json.JSONDecodeError, KeyError, TypeError) as error:
        sys.exit(
            f"refused: capabilities --json listed no grammars ({error!r}), exit {answer.code}, "
            f"stderr {answer.stderr[:300]!r}"
        )
    if cold:
        names = ", ".join(cold)
        sys.exit(f"refused: grammars not warmed: {names}. Copy them to <scratch>/grammars")
    return str(version)


def write_targets(args: argparse.Namespace) -> int:
    """Pick the targets of the first --limit records that have a checkout, into --out."""
    refuse_real_caches()
    # The scan runs git in each checkout: git status must not refresh its index.
    os.environ["GIT_OPTIONAL_LOCKS"] = "0"
    cold = set(grammars.ALL_LANGUAGES) - grammars.warmed_languages()
    if cold:
        sys.exit(f"refused: {GRAMMAR_OVERRIDE} holds no library for {', '.join(sorted(cold))}")
    records_path = args.records or args.harness / DEFAULT_RECORDS
    lines = records_path.read_text(encoding="utf-8").splitlines()
    records = [parse_record(line) for line in lines]
    present = [r for r in records if checkout(args.harness, r).is_dir()][: args.limit]
    extractor = TreeSitterExtractor()
    targets = [pick_target(extractor, args.harness, record) for record in present]
    args.out.write_text(json.dumps([asdict(t) for t in targets], indent=1), encoding="utf-8")
    print(f"{len(targets)} targets in {args.out}, from {len(records)} records")
    return 0


def write_dump(args: argparse.Namespace) -> int:
    """Ask the build of --targets every query, and write build, frame and answers to --out."""
    scratch, out = args.scratch.resolve(), args.out.resolve()
    env = environment(scratch)
    refuse_real_caches(env)
    if out.exists() and any(out.iterdir()):
        sys.exit(f"refused: {out} is not empty. Give each dump a new output directory")
    for directory in (scratch / "home", scratch / "tmp", scratch / "grammars", out / ANSWERS):
        directory.mkdir(parents=True, exist_ok=True)
    data = args.targets.read_bytes()
    targets = parse_targets(data.decode("utf-8"))
    prefix = tuple(args.prefix[1:] if args.prefix[:1] == ["--"] else args.prefix)
    if not prefix or not targets:
        sys.exit("refused: give a non-empty targets.json and the build's argv prefix after --")
    described = {"argv_prefix": list(prefix), "identity": identity(prefix)}
    build = Build(prefix, env, scratch, args.timeout)
    capabilities, _ = run_call(build, ["capabilities", "--json", "--repo", targets[0].root])
    described["version"] = warm_version(capabilities)
    (out / BUILD).write_text(json.dumps(described, indent=1), encoding="utf-8")
    (out / TARGETS).write_bytes(data)
    frames = _dump_answers(build, targets, out, args.workers)
    (out / FRAME).write_text(json.dumps(frames, indent=1), encoding="utf-8")
    return 0


def _dump_answers(
    build: Build, targets: Sequence[Target], out: Path, workers: int
) -> dict[str, Any]:
    frames: dict[str, Any] = {}
    calls: list[dict[str, Any]] = []
    with ThreadPoolExecutor(max_workers=workers) as pool:
        results = pool.map(partial(dump_target, build), targets)
        for done, result in enumerate(results, start=1):
            instance = result["instance_id"]
            answers = {"instance_id": instance, "answers": result["answers"]}
            (out / ANSWERS / f"{instance}.json").write_text(json.dumps(answers), encoding="utf-8")
            frames[instance] = result["frame"]
            calls.extend(result["answers"].values())
            print(f"[{done}/{len(targets)}] {instance}", flush=True)
    timeouts = sum(1 for call in calls if call["code"] is None)
    slowest = max(call["seconds"] for call in calls)
    print(f"{len(calls)} answers, {timeouts} timeouts, slowest {slowest} s", flush=True)
    return frames


def parse_args(argv: Sequence[str]) -> argparse.Namespace:
    """Return the parsed command line of either mode."""
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    modes = parser.add_subparsers(dest="mode", required=True)
    pick = modes.add_parser("targets", help="pick the targets under the base build")
    pick.add_argument("--harness", type=Path, required=True, help="the loc-bench-harness root")
    pick.add_argument("--records", type=Path, help="records JSONL (default: fix-base.jsonl)")
    pick.add_argument("--limit", type=int, help="keep the first N records with a checkout")
    pick.add_argument("--out", type=Path, required=True, help="the targets.json to write")
    pick.set_defaults(handler=write_targets)
    ask = modes.add_parser("dump", help="ask one build every query of a targets.json")
    ask.add_argument("--targets", type=Path, required=True, help="a targets.json file")
    ask.add_argument("--scratch", type=Path, required=True, help="this build's scratch root")
    ask.add_argument("--out", type=Path, required=True, help="a new output directory")
    ask.add_argument("--workers", type=int, default=DEFAULT_WORKERS, help="parallel targets")
    ask.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_SECONDS, help="per call")
    ask.add_argument("prefix", nargs=argparse.REMAINDER, help="-- and the build's argv prefix")
    ask.set_defaults(handler=write_dump)
    return parser.parse_args(argv)


if __name__ == "__main__":
    arguments = parse_args(sys.argv[1:])
    raise SystemExit(arguments.handler(arguments))
