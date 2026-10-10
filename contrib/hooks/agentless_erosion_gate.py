#!/usr/bin/env python3
"""PreToolUse hook for Write and Edit: run agentless-mcp's erosion checks on the proposed text."""

from __future__ import annotations

import contextlib
import hashlib
import json
import logging
import os
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import FrameType
from typing import BinaryIO, TypeVar

logger = logging.getLogger("agentless_erosion_gate")

ENV_GATE = "AGENTLESS_EROSION_GATE"
ENV_BIN = "AGENTLESS_EROSION_BIN"
ENV_LOG = "AGENTLESS_EROSION_LOG"
BINARY_NAME = "agentless-mcp"
CONFIG_FILENAME = ".agentless-mcp.json"
TEMP_PREFIX = "agentless-erosion-"
LOG_VERSION = 1

# Under the 15 s timeout in settings-example.json, so the hook ends a slow step itself
# and logs that step's reason before Claude Code sends SIGTERM.
DEADLINE_S = 10.0
# Held back from the time of every subprocess, so the log record is still written.
LOG_RESERVE_S = 0.5
# A git timeout ends the call as degraded, so a hung git costs one edit at most this long.
GIT_TIMEOUT_S = 2.0
MAX_PAYLOAD_BYTES = 4 * 1024 * 1024
MAX_CONFIG_BYTES = 64 * 1024
MAX_OLD_BYTES = 1_000_000
MAX_LISTED = 15
STDERR_TAIL_CHARS = 300

ALLOW = 0
BLOCK_EXIT = 2
USAGE_EXIT = 2

OFF = "off"
ADVISORY = "advisory"
BLOCK = "block"
MODES = (OFF, ADVISORY, BLOCK)
DEFAULT_MODE = ADVISORY
KNOWN_CHECKS = ("complexity_delta", "contract_drift", "clone_delta")
CONTRACT_DRIFT = "contract_drift"
CLONE_DELTA = "clone_delta"
# The gaps that lint emits by design, by check: the check is off for that file or run.
OFF_REASONS = {
    CONTRACT_DRIFT: ("no_boundary_globs", "no_rules_for_language"),
    CLONE_DELTA: ("no_tag_cache",),
}
# The input kinds lint accepts in the hook form: each check reads text pairs.
HOOK_FORM_INPUTS = ("pairs", "pairs+clones")
COVERAGE_CHECK = "coverage"
NOT_CHECKED = "not_checked"
SEVERITIES = ("advisory", "warning", NOT_CHECKED)

CLEAN = "clean"
ADVISED = "advisory"
BLOCKED = "blocked"
DEGRADED = "degraded"
SKIPPED = "skipped"
TERMINATED = "terminated"

WRITE = "Write"
EDIT = "Edit"
IDENTITY_KEYS = ("session_id", "transcript_path", "tool_use_id", "tool_name")

# The suffixes whose language the pair checks measure. A test pins this to the package.
TIER1_SUFFIXES = frozenset(
    {
        ".bash",
        ".c",
        ".cc",
        ".cjs",
        ".cpp",
        ".cxx",
        ".go",
        ".h",
        ".hpp",
        ".hxx",
        ".java",
        ".js",
        ".lua",
        ".mjs",
        ".py",
        ".rb",
        ".rs",
        ".sh",
        ".ts",
        ".tsx",
    }
)

BLOCK_HEADER = "Erosion gate blocked this edit:"
ADVISORY_HEADER = "Erosion gate findings for this edit (advisory, the edit is not blocked):"
ADVISORY_LABEL = "Advisory findings (these do not block):"

T = TypeVar("T")


class _HaltError(Exception):
    def __init__(self, outcome: str, reason: str) -> None:
        super().__init__(reason)
        self.outcome = outcome
        self.reason = reason


class _ShapeError(Exception):
    pass


class _TerminatedError(BaseException):
    pass


@dataclass(frozen=True)
class EditInput:
    """The replacement one Edit call asks for."""

    old_string: str
    new_string: str
    replace_all: bool


@dataclass(frozen=True)
class Payload:
    """One Write or Edit call, typed: ``change`` is Write content or an Edit replacement."""

    session_id: str
    transcript_path: str
    tool_use_id: str
    tool_name: str
    file_path: Path
    change: str | EditInput


@dataclass(frozen=True)
class Proposal:
    """The whole text a call would leave on disk, and the line-ending style an Edit chose."""

    text: str
    eol: str | None


@dataclass(frozen=True)
class LintFinding:
    """One row of the lint hook-form output."""

    check: str
    severity: str
    message: str
    path: str
    line: int
    evidence: str
    detail: dict[str, object] | None


@dataclass(frozen=True)
class LintOutput:
    """The one candidate the lint hook form reports, with ``observed`` as lint sent it."""

    findings: tuple[LintFinding, ...]
    observed: dict[str, object] | None


@dataclass(frozen=True)
class Verdict:
    """Lint's rows sorted by what the gate does with them."""

    outcome: str
    reason: str | None
    blocking: tuple[LintFinding, ...]
    advisory: tuple[LintFinding, ...]
    gaps: tuple[LintFinding, ...]
    off_by_config: tuple[str, ...]


@dataclass(frozen=True)
class Decision:
    """The exit code and the text a hook call prints."""

    code: int
    stdout: str = ""
    stderr: str = ""


@dataclass(frozen=True)
class _LintCall:
    binary: Path
    root: Path
    rel: str
    deadline: float


@dataclass(frozen=True)
class _Run:
    code: int
    stdout: bytes
    stderr: str


def main() -> int:
    """Gate one PreToolUse payload read from stdin and return the hook's exit code."""
    if os.environ.get(ENV_GATE) == OFF:
        return ALLOW
    start = time.monotonic()
    record = _blank_record()
    decision = _decide(record, start + DEADLINE_S)
    record["latency_ms"] = round((time.monotonic() - start) * 1000, 1)
    _append_log(record)
    return _emit(decision)


def _decide(record: dict[str, object], deadline: float) -> Decision:
    # The handler and the finally both ignore SIGTERM, so it can raise only once, inside here.
    try:
        signal.signal(signal.SIGTERM, _on_sigterm)
        try:
            return _gate(record, deadline)
        except Exception as error:
            logger.exception(
                "erosion gate internal error: tool_use_id=%s path=%s",
                record["tool_use_id"],
                record["path"],
            )
            return _halt(record, DEGRADED, f"internal error: {type(error).__name__}: {error}")
        finally:
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
    except _TerminatedError:
        return _halt(record, DEGRADED, TERMINATED)


def _gate(record: dict[str, object], deadline: float) -> Decision:
    try:
        payload = _read_payload(sys.stdin.buffer, record)
        resolved = Path(os.path.realpath(payload.file_path))
        record["path"] = str(resolved)
        _require_tier1(resolved)
        binary = _binary()
        record["binary"] = str(binary)
        root = _repository_root(payload.file_path, deadline)
        rel = _relative(resolved, root)
        record["root"], record["rel"] = str(root), rel
        modes = _modes(root, record)
        checks = _active(modes)
        old = _old_text(resolved)
        if old is not None:
            record["old_sha256"] = hashlib.sha256(old.encode("utf-8")).hexdigest()
        proposal = _proposal(payload, old)
        record["eol"] = proposal.eol
        output = _lint(_LintCall(binary, root, rel, deadline), proposal, checks, record)
    except _HaltError as halt:
        return _halt(record, halt.outcome, halt.reason)
    return _decision(classify(output, modes), modes, output, record)


def _read_payload(stream: BinaryIO, record: dict[str, object]) -> Payload:
    raw = stream.read(MAX_PAYLOAD_BYTES + 1)
    if len(raw) > MAX_PAYLOAD_BYTES:
        raise _HaltError(DEGRADED, "malformed payload: over 4 MiB")
    try:
        document = json.loads(raw.decode("utf-8"))
    except ValueError as error:
        raise _HaltError(DEGRADED, f"malformed payload: {error}") from error
    record.update(_identity(document))
    try:
        return parse_payload(document)
    except _ShapeError as error:
        raise _HaltError(DEGRADED, f"malformed payload: {error}") from error


def _identity(document: object) -> dict[str, object]:
    if not isinstance(document, dict):
        return {}
    return {key: document[key] for key in IDENTITY_KEYS if isinstance(document.get(key), str)}


def parse_payload(document: object) -> Payload:
    """Return the typed Write or Edit call in a PreToolUse payload, or raise on a malformed one."""
    if not isinstance(document, dict):
        message = "the payload is not a JSON object"
        raise _ShapeError(message)
    tool_name = _field(document, "tool_name", str, "payload")
    if tool_name not in (WRITE, EDIT):
        raise _HaltError(SKIPPED, f"not a Write or Edit call: {tool_name}")
    tool_input = _field(document, "tool_input", dict, "payload")
    file_path = Path(_field(tool_input, "file_path", str, "tool_input"))
    if not file_path.is_absolute():
        file_path = Path(_field(document, "cwd", str, "payload")) / file_path
    return Payload(
        session_id=_field(document, "session_id", str, "payload"),
        transcript_path=_field(document, "transcript_path", str, "payload"),
        tool_use_id=_field(document, "tool_use_id", str, "payload"),
        tool_name=tool_name,
        file_path=file_path,
        change=_change(tool_name, tool_input),
    )


def _change(tool_name: str, tool_input: Mapping[str, object]) -> str | EditInput:
    if tool_name == WRITE:
        return _field(tool_input, "content", str, "tool_input")
    # Claude Code fills the schema default before the hook runs, but the model may omit it.
    replace_all = False
    if "replace_all" in tool_input:
        replace_all = _field(tool_input, "replace_all", bool, "tool_input")
    return EditInput(
        old_string=_field(tool_input, "old_string", str, "tool_input"),
        new_string=_field(tool_input, "new_string", str, "tool_input"),
        replace_all=replace_all,
    )


def _field(document: Mapping[str, object], key: str, kind: type[T], where: str) -> T:
    if key not in document:
        message = f"{where}.{key} is missing"
        raise _ShapeError(message)
    value = document[key]
    if not isinstance(value, kind) or (kind is int and isinstance(value, bool)):
        message = f"{where}.{key} is not a {kind.__name__}"
        raise _ShapeError(message)
    return value


def _require_tier1(resolved: Path) -> None:
    if resolved.suffix not in TIER1_SUFFIXES:
        raise _HaltError(SKIPPED, f"not a tier-1 source file: {resolved.name}")


def _binary() -> Path:
    configured = os.environ.get(ENV_BIN)
    if configured:
        path = Path(configured)
        if not (path.is_file() and os.access(path, os.X_OK)):
            raise _HaltError(SKIPPED, f"{ENV_BIN} does not name an executable file: {configured}")
        return path
    found = shutil.which(BINARY_NAME)
    if found is None:
        raise _HaltError(SKIPPED, f"{BINARY_NAME} not found on PATH")
    return Path(found)


def _repository_root(given: Path, deadline: float) -> Path:
    # The root comes from where the caller writes, and containment is tested on the resolved file.
    anchor = given.parent
    while not anchor.is_dir():
        anchor = anchor.parent
    budget = min(GIT_TIMEOUT_S, deadline - time.monotonic() - LOG_RESERVE_S)
    if budget <= 0:
        raise _HaltError(DEGRADED, "no time left for git")
    try:
        completed = subprocess.run(
            ["git", "-C", str(anchor), "rev-parse", "--show-toplevel"],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=budget,
            check=False,
            env={**os.environ, "LC_ALL": "C"},
        )
    except subprocess.TimeoutExpired:
        raise _HaltError(DEGRADED, f"git rev-parse timed out after {budget:.1f} s") from None
    except OSError as error:
        raise _HaltError(DEGRADED, f"cannot run git: {error}") from error
    stderr = completed.stderr.decode("utf-8", errors="replace")
    if completed.returncode != 0 and "not a git repository" in stderr:
        raise _HaltError(SKIPPED, "not in a git repository")
    if completed.returncode != 0:
        raise _HaltError(
            DEGRADED, f"git rev-parse exited {completed.returncode}: {_last_line(stderr)}"
        )
    return Path(os.path.realpath(completed.stdout.decode("utf-8").strip()))


def _relative(resolved: Path, root: Path) -> str:
    if not resolved.is_relative_to(root):
        raise _HaltError(SKIPPED, "the resolved target is outside the repository")
    return resolved.relative_to(root).as_posix()


def _modes(root: Path, record: dict[str, object]) -> dict[str, str]:
    document, warnings = _config_document(root)
    entries, entry_warnings = repo_modes(document)
    modes, env_warnings = resolve_modes(entries, os.environ.get(ENV_GATE))
    record["modes"] = modes
    record["config_warnings"] = [*warnings, *entry_warnings, *env_warnings]
    return modes


def _config_document(root: Path) -> tuple[dict[str, object] | None, list[str]]:
    try:
        with (root / CONFIG_FILENAME).open("rb") as handle:
            raw = handle.read(MAX_CONFIG_BYTES + 1)
    except FileNotFoundError:
        return None, []
    except OSError as error:
        return None, [f"{CONFIG_FILENAME} was not read: {error}"]
    if len(raw) > MAX_CONFIG_BYTES:
        return None, [f"{CONFIG_FILENAME} is over {MAX_CONFIG_BYTES} bytes and was ignored"]
    try:
        document = json.loads(raw.decode("utf-8"))
    except ValueError as error:
        return None, [f"{CONFIG_FILENAME} is not JSON and was ignored: {error}"]
    if not isinstance(document, dict):
        return None, [f"{CONFIG_FILENAME} is not a JSON object and was ignored"]
    return document, []


def repo_modes(document: Mapping[str, object] | None) -> tuple[dict[str, str], list[str]]:
    """Return the valid ``erosion_gate`` entries of a config document, and a warning per bad one."""
    if document is None or "erosion_gate" not in document:
        return {}, []
    value = document["erosion_gate"]
    if not isinstance(value, dict):
        return {}, ["erosion_gate is not an object and was ignored"]
    entries: dict[str, str] = {}
    warnings: list[str] = []
    for check in (check for check in KNOWN_CHECKS if check in value):
        if value[check] in MODES:
            entries[check] = value[check]
        else:
            warnings.append(
                f"erosion_gate.{check} is {value[check]!r}, not off, advisory, or block, "
                "and was ignored"
            )
    return entries, warnings


def resolve_modes(
    entries: Mapping[str, str], env_value: str | None
) -> tuple[dict[str, str], list[str]]:
    """Return each check's mode: the repository entry, then the environment, then advisory."""
    default = DEFAULT_MODE
    warnings: list[str] = []
    if env_value:
        if env_value in (ADVISORY, BLOCK):
            default = env_value
        else:
            warnings.append(
                f"{ENV_GATE}={env_value!r} is not off, advisory, or block, so advisory applies"
            )
    return {check: entries.get(check, default) for check in KNOWN_CHECKS}, warnings


def _active(modes: Mapping[str, str]) -> list[str]:
    checks = [check for check in KNOWN_CHECKS if modes[check] != OFF]
    if not checks:
        raise _HaltError(SKIPPED, "every check is off")
    return checks


def _old_text(resolved: Path) -> str | None:
    try:
        status = resolved.stat()
    except FileNotFoundError:
        return None
    except OSError as error:
        raise _HaltError(DEGRADED, f"cannot stat the target: {error}") from error
    if not stat.S_ISREG(status.st_mode):
        raise _HaltError(SKIPPED, "the target is not a regular file")
    try:
        with resolved.open("rb") as handle:
            data = handle.read(MAX_OLD_BYTES + 1)
    except OSError as error:
        raise _HaltError(DEGRADED, f"cannot read the old text: {error}") from error
    if len(data) > MAX_OLD_BYTES:
        raise _HaltError(DEGRADED, f"the old text is over {MAX_OLD_BYTES:,} bytes")
    try:
        return data.decode("utf-8")
    except UnicodeDecodeError as error:
        raise _HaltError(DEGRADED, f"the old text is not UTF-8: byte {error.start}") from error


def _proposal(payload: Payload, old: str | None) -> Proposal:
    if isinstance(payload.change, str):
        return Proposal(payload.change, None)
    if old is None:
        raise _HaltError(SKIPPED, "Edit on a missing file, which Claude Code fails")
    return edited_text(old, payload.change)


def edited_text(old: str, edit: EditInput) -> Proposal:
    """Return the text Claude Code's Edit writes, or raise when Claude Code would fail the edit."""
    crlf = old.count("\r\n")
    lf = old.count("\n") - crlf
    normalized = old.replace("\r\n", "\n")
    _check_edit(normalized, edit)
    new = normalized.replace(edit.old_string, edit.new_string, -1 if edit.replace_all else 1)
    if crlf > lf:
        return Proposal(new.replace("\n", "\r\n"), "crlf")
    # A tie gets LF. Line endings change no metric, so this choice cannot move a finding.
    return Proposal(new, "tie" if crlf == lf and crlf > 0 else "lf")


def _check_edit(normalized: str, edit: EditInput) -> None:
    if not edit.old_string:
        raise _HaltError(SKIPPED, "Edit with an empty old_string, which Claude Code fails")
    if edit.old_string == edit.new_string:
        raise _HaltError(SKIPPED, "old_string equals new_string, which Claude Code fails")
    occurrences = normalized.count(edit.old_string)
    if occurrences == 0:
        raise _HaltError(SKIPPED, "old_string is not in the file, so Claude Code fails this edit")
    if occurrences > 1 and not edit.replace_all:
        raise _HaltError(
            SKIPPED,
            f"old_string occurs {occurrences} times without replace_all, "
            "so Claude Code fails this edit",
        )


def _lint(
    call: _LintCall, proposal: Proposal, checks: Sequence[str], record: dict[str, object]
) -> LintOutput:
    try:
        data = proposal.text.encode("utf-8")
    except UnicodeEncodeError as error:
        raise _HaltError(DEGRADED, f"the proposed text is not UTF-8: {error.reason}") from error
    record["new_sha256"] = hashlib.sha256(data).hexdigest()
    with _scratch_file(data) as after:
        return _run_lint(call, after, checks, record)


@contextlib.contextmanager
def _scratch_file(data: bytes) -> Iterator[Path]:
    created: list[Path] = []
    try:
        try:
            with _sigterm_deferred():
                descriptor, name = tempfile.mkstemp(prefix=TEMP_PREFIX)
                created.append(Path(name))
            with os.fdopen(descriptor, "wb") as handle:
                handle.write(data)
        except OSError as error:
            raise _HaltError(DEGRADED, f"cannot write the temp file: {error}") from error
        yield created[0]
    finally:
        for path in created:
            path.unlink(missing_ok=True)


@contextlib.contextmanager
def _sigterm_deferred() -> Iterator[None]:
    # A signal between mkstemp and recording its path would leave the file behind.
    if not hasattr(signal, "pthread_sigmask"):
        yield
        return
    signal.pthread_sigmask(signal.SIG_BLOCK, {signal.SIGTERM})
    try:
        yield
    finally:
        signal.pthread_sigmask(signal.SIG_UNBLOCK, {signal.SIGTERM})


def _run_lint(
    call: _LintCall, after: Path, checks: Sequence[str], record: dict[str, object]
) -> LintOutput:
    record["checks"] = list(checks)
    run = _invoke(call, _lint_arguments(call, after, checks), "lint")
    if run.code == USAGE_EXIT:
        checks = _retry_checks(checks, _pair_checks(call, run), run, record)
        record["checks"] = list(checks)
        run = _invoke(call, _lint_arguments(call, after, checks), "lint")
    if run.code != 0:
        raise _HaltError(DEGRADED, _failure("lint", run))
    try:
        return parse_lint_output(run.stdout, checks)
    except _ShapeError as error:
        raise _HaltError(DEGRADED, f"lint output: {error}") from error


def _lint_arguments(call: _LintCall, after: Path, checks: Sequence[str]) -> list[str]:
    return [
        "lint",
        "--repo",
        str(call.root),
        "--path",
        call.rel,
        "--after",
        str(after),
        "--json",
        "--only",
        ",".join(checks),
    ]


def _invoke(call: _LintCall, arguments: Sequence[str], what: str) -> _Run:
    budget = call.deadline - time.monotonic() - LOG_RESERVE_S
    if budget <= 0:
        raise _HaltError(DEGRADED, f"no time left for {what}")
    try:
        completed = subprocess.run(
            [str(call.binary), *arguments],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            timeout=budget,
            check=False,
        )
    except subprocess.TimeoutExpired:
        raise _HaltError(DEGRADED, f"{what} timed out after {budget:.1f} s") from None
    except OSError as error:
        raise _HaltError(DEGRADED, f"cannot run {call.binary}: {error}") from error
    stderr = completed.stderr.decode("utf-8", errors="replace")
    return _Run(completed.returncode, completed.stdout, stderr)


def _pair_checks(call: _LintCall, refused: _Run) -> frozenset[str]:
    run = _invoke(call, ["capabilities", "--repo", str(call.root), "--json"], "capabilities")
    if run.code != 0:
        raise _HaltError(DEGRADED, _failure("capabilities", run))
    try:
        listed = parse_lint_checks(run.stdout)
    except _ShapeError as error:
        raise _HaltError(DEGRADED, f"capabilities output: {error}") from error
    if listed is None and lacks_hook_form(refused.stderr):
        raise _HaltError(DEGRADED, f"{BINARY_NAME} lacks the hook form")
    if listed is None:
        reason = f"capabilities has no lint_checks, and lint said: {_last_line(refused.stderr)}"
        raise _HaltError(DEGRADED, reason)
    return frozenset(name for name, needs in listed.items() if needs in HOOK_FORM_INPUTS)


def parse_lint_checks(stdout: bytes) -> dict[str, str] | None:
    """Return each check that ``capabilities --json`` lists, with its input, or None for none."""
    document = _json_object(stdout)
    if "lint_checks" not in document:
        return None
    rows = _field(document, "lint_checks", list, "output")
    listed: dict[str, str] = {}
    for index, row in enumerate(rows):
        where = f"lint_checks[{index}]"
        if not isinstance(row, dict):
            message = f"{where} is not an object"
            raise _ShapeError(message)
        name = _field(row, "name", str, where)
        if name in listed:
            message = f"{where}.name {name!r} appears twice"
            raise _ShapeError(message)
        listed[name] = _field(row, "input", str, where)
    return listed


def _retry_checks(
    checks: Sequence[str], pairs: frozenset[str], refused: _Run, record: dict[str, object]
) -> list[str]:
    kept = [check for check in checks if check in pairs]
    if not kept:
        raise _HaltError(DEGRADED, f"{BINARY_NAME} offers none of the gated checks")
    if len(kept) == len(checks):
        # Every requested check is offered, so the refusal has another cause and a retry repeats it.
        raise _HaltError(DEGRADED, _failure("lint", refused))
    record["dropped"] = [
        {"check": check, "reason": "not a pairs check in lint_checks"}
        for check in checks
        if check not in pairs
    ]
    return kept


def _failure(what: str, run: _Run) -> str:
    if run.code == USAGE_EXIT:
        return f"{what} usage error: {_last_line(run.stderr)}"
    return f"{what} exited {run.code}: {run.stderr[-STDERR_TAIL_CHARS:].strip()}"


def lacks_hook_form(stderr: str) -> bool:
    """True when a lint usage error comes from a release whose usage line has no ``--path``."""
    usage, _, _ = stderr.partition(": error:")
    return "usage:" in usage and "--path" not in usage


def parse_lint_output(stdout: bytes, checks: Sequence[str]) -> LintOutput:
    """Return the lint hook-form JSON as typed rows, or raise on any other shape."""
    document = _json_object(stdout)
    candidates = _field(document, "candidates", list, "output")
    if len(candidates) != 1 or not isinstance(candidates[0], dict):
        message = f"output.candidates holds {len(candidates)} items, not one object"
        raise _ShapeError(message)
    candidate = candidates[0]
    findings = _field(candidate, "findings", list, "candidate")
    observed = candidate.get("observed")
    if observed is not None and not isinstance(observed, dict):
        message = "candidate.observed is not an object"
        raise _ShapeError(message)
    allowed = {*checks, COVERAGE_CHECK}
    rows = tuple(
        _finding(item, f"findings[{index}]", allowed) for index, item in enumerate(findings)
    )
    return LintOutput(rows, observed)


def _json_object(stdout: bytes) -> dict[str, object]:
    try:
        document = json.loads(stdout.decode("utf-8"))
    except ValueError as error:
        message = f"not JSON: {error}"
        raise _ShapeError(message) from error
    if not isinstance(document, dict):
        message = "not a JSON object"
        raise _ShapeError(message)
    return document


def _finding(item: object, where: str, allowed: set[str]) -> LintFinding:
    if not isinstance(item, dict):
        message = f"{where} is not an object"
        raise _ShapeError(message)
    check = _field(item, "check", str, where)
    severity = _field(item, "severity", str, where)
    if check not in allowed:
        message = f"{where}.check {check!r} was not requested"
        raise _ShapeError(message)
    if severity not in SEVERITIES or (check == COVERAGE_CHECK and severity != NOT_CHECKED):
        message = f"{where}.severity {severity!r} is not valid for {check}"
        raise _ShapeError(message)
    detail = item.get("detail")
    if detail is not None and not isinstance(detail, dict):
        message = f"{where}.detail is not an object"
        raise _ShapeError(message)
    return LintFinding(
        check=check,
        severity=severity,
        message=_field(item, "message", str, where),
        path=_field(item, "path", str, where),
        line=_field(item, "line", int, where),
        evidence=_field(item, "evidence", str, where),
        detail=detail,
    )


def classify(output: LintOutput, modes: Mapping[str, str]) -> Verdict:
    """Sort lint's rows into blocking and advisory findings, gaps, and checks off by design."""
    blocking: list[LintFinding] = []
    advisory: list[LintFinding] = []
    gaps: list[LintFinding] = []
    off: list[str] = []
    for finding in output.findings:
        if finding.severity == NOT_CHECKED and _off_by_config(finding):
            off.append(finding.check)
        elif finding.severity == NOT_CHECKED:
            gaps.append(finding)
        elif modes[finding.check] == BLOCK:
            blocking.append(finding)
        else:
            advisory.append(finding)
    outcome, reason = _outcome(blocking, advisory, gaps)
    return Verdict(outcome, reason, tuple(blocking), tuple(advisory), tuple(gaps), tuple(off))


def _off_by_config(finding: LintFinding) -> bool:
    detail = finding.detail
    return (
        detail is not None
        and "reason" in detail
        and detail["reason"] in OFF_REASONS.get(finding.check, ())
    )


def _outcome(
    blocking: Sequence[LintFinding], advisory: Sequence[LintFinding], gaps: Sequence[LintFinding]
) -> tuple[str, str | None]:
    if blocking:
        return BLOCKED, None
    if advisory:
        return ADVISED, None
    if gaps:
        return DEGRADED, gaps[0].message
    return CLEAN, None


def _decision(
    verdict: Verdict, modes: Mapping[str, str], output: LintOutput, record: dict[str, object]
) -> Decision:
    record["outcome"], record["reason"] = verdict.outcome, verdict.reason
    record["gaps"] = [{"check": gap.check, "message": gap.message} for gap in verdict.gaps]
    record["off_by_config"] = list(verdict.off_by_config)
    record["findings"] = [
        _logged(finding, modes[finding.check])
        for finding in output.findings
        if finding.severity != NOT_CHECKED
    ]
    record["observed"] = output.observed
    if verdict.blocking:
        return Decision(BLOCK_EXIT, stderr=findings_text(verdict.blocking, verdict.advisory, modes))
    if verdict.advisory:
        context = findings_text((), verdict.advisory, modes)
        output_json = {"hookEventName": "PreToolUse", "additionalContext": context}
        return Decision(ALLOW, stdout=json.dumps({"hookSpecificOutput": output_json}))
    return Decision(ALLOW)


def _logged(finding: LintFinding, mode: str) -> dict[str, object]:
    return {
        "check": finding.check,
        "severity": finding.severity,
        "mode": mode,
        "path": finding.path,
        "line": finding.line,
        "message": finding.message,
        "evidence": finding.evidence,
        "detail": finding.detail,
    }


def findings_text(
    blocking: Sequence[LintFinding], advisory: Sequence[LintFinding], modes: Mapping[str, str]
) -> str:
    """Return the text the model reads: blocking findings first, at most 15 lines, then the mode."""
    listed_blocking = list(blocking[:MAX_LISTED])
    listed_advisory = list(advisory[: MAX_LISTED - len(listed_blocking)])
    hidden = len(blocking) + len(advisory) - len(listed_blocking) - len(listed_advisory)
    lines = [BLOCK_HEADER if blocking else ADVISORY_HEADER]
    lines.extend(_line(finding) for finding in listed_blocking)
    if blocking and listed_advisory:
        lines.append(ADVISORY_LABEL)
    lines.extend(_line(finding) for finding in listed_advisory)
    if hidden:
        lines.append(f"...and {hidden} more findings")
    lines.append(_mode_line(modes))
    return "\n".join(lines)


def _line(finding: LintFinding) -> str:
    return f"{finding.path}:{finding.line}: {finding.message}"


def _mode_line(modes: Mapping[str, str]) -> str:
    current = ", ".join(f"{check} {modes[check]}" for check in KNOWN_CHECKS)
    return (
        f"Erosion gate mode: {current}. Change it with the erosion_gate key in "
        f"{CONFIG_FILENAME} or the {ENV_GATE} variable (off, advisory, block)."
    )


def _halt(record: dict[str, object], outcome: str, reason: str) -> Decision:
    record["outcome"], record["reason"] = outcome, reason
    return Decision(ALLOW)


def _on_sigterm(_signum: int, _frame: FrameType | None) -> None:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    raise _TerminatedError


def _blank_record() -> dict[str, object]:
    return {
        "v": LOG_VERSION,
        "ts": time.time(),
        "latency_ms": None,
        "session_id": None,
        "transcript_path": None,
        "tool_use_id": None,
        "tool_name": None,
        "path": None,
        "root": None,
        "rel": None,
        "binary": None,
        "modes": None,
        "checks": [],
        "dropped": [],
        "off_by_config": [],
        "config_warnings": [],
        "old_sha256": None,
        "new_sha256": None,
        "eol": None,
        "outcome": None,
        "reason": None,
        "gaps": [],
        "findings": [],
        "observed": None,
    }


def _append_log(record: dict[str, object]) -> None:
    target = os.environ.get(ENV_LOG)
    if not target:
        return
    try:
        line = (json.dumps(record) + "\n").encode("utf-8")
        path = Path(target)
        path.parent.mkdir(parents=True, exist_ok=True)
        descriptor = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
        try:
            written = os.write(descriptor, line)
        finally:
            os.close(descriptor)
    except (OSError, TypeError, ValueError):
        logger.exception(
            "erosion gate cannot append to %s: tool_use_id=%s", target, record["tool_use_id"]
        )
        return
    if written != len(line):
        logger.error(
            "erosion gate wrote %d of %d bytes to %s: tool_use_id=%s",
            written,
            len(line),
            target,
            record["tool_use_id"],
        )


def _emit(decision: Decision) -> int:
    if decision.stdout:
        sys.stdout.write(decision.stdout + "\n")
    if decision.stderr:
        sys.stderr.write(decision.stderr + "\n")
    return decision.code


def _last_line(text: str) -> str:
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    return lines[-1][-STDERR_TAIL_CHARS:] if lines else "(no output)"


if __name__ == "__main__":
    raise SystemExit(main())
