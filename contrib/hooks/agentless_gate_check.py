#!/usr/bin/env python3
"""PreToolUse hook: gate broad native search on structural localization.

This is the enforcing half of the structural-first gate. It reads the marker
that ``agentless_gate_mark.py`` writes for the calling session:

* marker present -- exit 0, and the call proceeds;
* an exact-file ``Grep`` -- exit 0 even without a marker, because the caller
  has already localized the search;
* a ``Bash`` command that does not parse as a tree search -- exit 0, which is
  most of them: a ``grep`` in a shell session usually filters the output of
  the command before it;
* any broader ``Grep``, ``Glob`` or tree-searching ``Bash`` command without a
  marker -- exit 2, which blocks the call and returns this hook's stderr as a
  just-in-time instruction.

Search reaches this hook in two shapes and both are covered. A native tool
names itself. A shell tool carries the search as a string, so ``Bash`` is read
for the search rather than trusted for its name -- a harness that instructs
the model to search with ``grep`` or ``rg`` inside ``Bash`` produces payloads
whose tool name is always ``Bash``, and keying on ``Grep``/``Glob`` alone left
that whole mode ungated.

The gate fires once per session. ``agentless_gate_mark.py`` writes the marker
only after an operation that localizes code structure; diagnostics and raw
reads do not unlock broad search. The constraint is a scope-aware order, not
a ban.

Any internal failure exits 0. A gate that breaks an unrelated session is worse
than a gate that misses one call, and a hook cannot tell "this stdin is
malformed" from "this session is not the one I was written for". A payload
that carries no session id is allowed for the same reason: no marker can ever
be written for that call, so a denial there would deny every later call too.

Set ``AGENTLESS_GATE_LOG`` to a file path to append one JSONL line per
decision. The variable is optional, and it is unset by default.

This script imports the standard library only. It is not part of the
``agentless_mcp`` package, and Claude Code runs it as a shell command, so it
must not need the package on ``sys.path``.
"""

from __future__ import annotations

import contextlib
import hashlib
import json
import os
import shlex
import sys
import time
from pathlib import Path

# Session state, not user data: the marker holds only the unlock receipt and is
# meant to disappear when the machine reboots. Keep this path identical in both hooks.
MARKER_DIR = Path("/tmp/agentless_gate")
LOG_ENV = "AGENTLESS_GATE_LOG"

ALLOW = 0
DENY = 2

# The tools this gate governs. Anything else reaching this hook was matched by
# a settings entry wider than the gate, and is allowed unchanged.
#
# Two families, because the search arrives in two shapes. A native tool names
# itself, so the tool name is the whole decision. A shell tool carries the
# search as a string, so the command has to be read. Keying on the native
# names alone left the second shape ungated: a harness that tells the model to
# search with `grep` inside `Bash` never reached this hook at all.
NATIVE_SEARCH_TOOLS = frozenset({"Grep", "Glob"})
# ``Bash`` is the canonical shell-tool name in both Claude Code and Codex
# payloads. ``exec_command`` is Codex's unified-exec wrapper, which reports its
# own name and carries the same ``tool_input.command`` string; without it here
# a client that enables unified exec routes every search past the gate.
SHELL_TOOLS = frozenset({"Bash", "exec_command"})
SEARCH_TOOLS = NATIVE_SEARCH_TOOLS | SHELL_TOOLS

DENY_MESSAGE = """Structural pass first: this session has not localized with agentless yet.

Call mcp__agentless__orient with operation="map" and focus seeds from the task \
(file paths or symbol names). Escalate with mcp__agentless__symbols \
operation="expand" on the stable ids it returns.

Broad text search unlocks after that call, whether it runs as Grep, Glob, or \
grep/rg/find inside Bash. A Grep scoped to one existing file is already \
allowed, and so is a shell command that filters another command's output. Use \
broad search afterwards for string literals, error messages, config keys, and \
test names.
"""


def append_log(entry: dict[str, object]) -> None:
    """Append *entry* to ``AGENTLESS_GATE_LOG``. Do nothing when the var is unset."""
    path = os.environ.get(LOG_ENV)
    if not path:
        return
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    with target.open("a") as handle:
        handle.write(json.dumps(entry) + "\n")


def marker_path(session_id: str) -> Path:
    """Return the fixed-width marker path for an untrusted session id."""
    digest = hashlib.sha256(session_id.encode()).hexdigest()
    return MARKER_DIR / f"{digest}.json"


def _exact_file_grep(payload: dict[str, object]) -> bool:
    """True when this Grep is explicitly scoped to one existing file."""
    if payload.get("tool_name") != "Grep":
        return False
    tool_input = payload.get("tool_input")
    if not isinstance(tool_input, dict):
        return False
    raw_path = tool_input.get("path")
    if not isinstance(raw_path, str) or not raw_path:
        return False
    path = Path(raw_path)
    raw_cwd = payload.get("cwd")
    cwd = Path(raw_cwd) if isinstance(raw_cwd, str) and raw_cwd else Path.cwd()
    candidate = path if path.is_absolute() else cwd / path
    return candidate.is_file()


# Commands whose whole job is to search a tree. `grep` needs an explicit
# recursive flag to walk one; `rg` walks the working directory by default, so
# the two are classified by different rules below.
_GREP_NAMES = frozenset({"grep", "egrep", "fgrep"})
_RG_NAMES = frozenset({"rg", "ripgrep"})
_FIND_NAMES = frozenset({"find", "fd", "fdfind"})
_RECURSIVE_FLAGS = frozenset({"-r", "-R", "--recursive", "--dereference-recursive"})
_FIND_SEARCH_FLAGS = frozenset({"-name", "-iname", "-path", "-ipath", "-regex", "-iregex"})
# Splitting a command line here, not running one: these separate one command
# from the next. A segment after `|` is a filter over the previous command's
# output, which touches no repository file and is never denied.
_PIPE = "|"
_SEPARATORS = frozenset({"|", "||", "&&", ";", "&"})


def _segments(tokens: list[str]) -> list[tuple[bool, list[str]]]:
    """Split tokens into (is_pipe_filter, argv) commands.

    ``is_pipe_filter`` marks a command reading the previous one's stdout. A
    ``grep`` there filters console output rather than searching the tree, and
    that is the common case in a shell session -- denying it would spend the
    gate's whole credibility on false positives.
    """
    out: list[tuple[bool, list[str]]] = []
    current: list[str] = []
    piped = False
    for token in tokens:
        if token in _SEPARATORS:
            out.append((piped, current))
            piped = token == _PIPE
            current = []
            continue
        current.append(token)
    out.append((piped, current))
    return [(is_pipe, argv) for is_pipe, argv in out if argv]


def _paths_after_pattern(argv: list[str]) -> list[str]:
    """Return the operands a search command was pointed at, pattern excluded."""
    operands = [t for t in argv[1:] if not t.startswith("-")]
    return operands[1:]


def _bundled_recursive(token: str) -> bool:
    """True for a short cluster such as ``-rn``.

    Long options are excluded on purpose: ``--regexp`` contains an ``r`` and
    means nothing of the sort.
    """
    return (
        token.startswith("-")
        and not token.startswith("--")
        and len(token) > 1
        and any(flag in token[1:] for flag in ("r", "R"))
    )


def _grep_walks_a_tree(argv: list[str]) -> bool:
    """``grep`` reads stdin or its file operands; only ``-r`` makes it recurse."""
    return any(t in _RECURSIVE_FLAGS or _bundled_recursive(t) for t in argv[1:])


def _rg_walks_a_tree(argv: list[str], cwd: Path) -> bool:
    """``rg`` walks the working directory unless pointed at a file."""
    paths = _paths_after_pattern(argv)
    return not paths or any((cwd / p).is_dir() for p in paths)


def _searches_a_tree(argv: list[str], cwd: Path) -> bool:
    """True only when *argv* parses cleanly as a search over a directory tree.

    Every uncertain shape returns False. An operand resolving to neither an
    existing file nor an existing directory is doubt, not evidence: the tokens
    after a value-taking flag cannot be told from real operands without
    reimplementing each tool's option table.
    """
    name = Path(argv[0]).name
    if name in _GREP_NAMES:
        return _grep_walks_a_tree(argv)
    if name in _RG_NAMES:
        return _rg_walks_a_tree(argv, cwd)
    if name in _FIND_NAMES:
        return any(token in _FIND_SEARCH_FLAGS for token in argv[1:])
    return False


def broad_shell_search(command: str, cwd: Path) -> bool:
    """True when *command* unambiguously searches the repository tree.

    Shell text cannot be parsed in general -- quoting, ``xargs``, subshells and
    command substitution all defeat it -- so this answers only the question it
    can answer honestly, and every other shape reads as False. Known shapes
    that slip through and are accepted as the cost of that posture: ``git
    grep``, a search built by ``xargs`` or a subshell, and any command whose
    name reaches the shell through a variable.
    """
    try:
        tokens = shlex.split(command)
    except ValueError:
        # Unbalanced quoting. The command may not even run; do not guess.
        return False
    return any(
        not is_pipe_filter and _searches_a_tree(argv, cwd)
        for is_pipe_filter, argv in _segments(tokens)
    )


def _payload_cwd(payload: dict[str, object]) -> Path:
    """Return the directory a relative operand in this payload resolves against."""
    raw_cwd = payload.get("cwd")
    return Path(raw_cwd) if isinstance(raw_cwd, str) and raw_cwd else Path.cwd()


def _marker_storage_ready() -> bool:
    """Prove session state can be written before enforcing its absence."""
    probe = MARKER_DIR / f".write-probe-{os.getpid()}"
    try:
        MARKER_DIR.mkdir(parents=True, exist_ok=True)
        probe.write_text("", encoding="utf-8")
        probe.unlink()
    except OSError:
        return False
    return True


def decide() -> int:
    """Return the exit code for one PreToolUse payload read from stdin."""
    payload = json.loads(sys.stdin.read() or "{}")
    if not isinstance(payload, dict):
        append_log(
            {
                "ts": time.time(),
                "session_id": None,
                "event": "native_search_allowed",
                "reason": "malformed_payload",
            }
        )
        return ALLOW
    session_id = payload.get("session_id")
    tool_input = payload.get("tool_input")
    entry: dict[str, object] = {
        "ts": time.time(),
        "session_id": session_id,
        "tool": payload.get("tool_name"),
        "pattern": tool_input.get("pattern") if isinstance(tool_input, dict) else None,
    }
    # No session id means no marker can ever be written for this call, so a
    # denial here would deny every later call too. Fail open and say so.
    if not isinstance(session_id, str) or not session_id:
        append_log({**entry, "event": "native_search_allowed", "reason": "no_session_id"})
        return ALLOW
    tool_name = payload.get("tool_name")
    # A tool this gate does not govern and a search whose payload cannot be
    # read are both allowed, and they are different events. One says the hook
    # was matched too widely; the other says a Grep went unexamined.
    if tool_name not in SEARCH_TOOLS:
        append_log({**entry, "event": "native_search_allowed", "reason": "not_a_search_tool"})
        return ALLOW
    if tool_name in SHELL_TOOLS:
        return _decide_shell(payload, session_id, entry)
    return _decide_native(payload, session_id, entry, str(tool_name))


def _decide_native(
    payload: dict[str, object], session_id: str, entry: dict[str, object], tool_name: str
) -> int:
    """Decide one ``Grep`` or ``Glob`` payload, where the tool names itself."""
    tool_input = payload.get("tool_input")
    pattern = tool_input.get("pattern") if isinstance(tool_input, dict) else None
    if not isinstance(pattern, str) or not pattern:
        append_log({**entry, "event": "native_search_allowed", "reason": "malformed_payload"})
        return ALLOW
    allow_reason = None
    if marker_path(session_id).exists():
        allow_reason = "already_unlocked"
    elif _exact_file_grep(payload):
        allow_reason = "exact_file_scope"
    elif not _marker_storage_ready():
        allow_reason = "marker_storage_unavailable"
    if allow_reason is not None:
        append_log({**entry, "event": "native_search_allowed", "reason": allow_reason})
        return ALLOW
    reason = "glob_before_structure" if tool_name == "Glob" else "broad_grep_before_structure"
    append_log({**entry, "event": "denied", "reason": reason})
    sys.stderr.write(DENY_MESSAGE)
    return DENY


def _decide_shell(payload: dict[str, object], session_id: str, entry: dict[str, object]) -> int:
    """Decide one shell-tool payload, where the search is a string not a tool.

    A session that routes search through the shell reaches the gate as one
    opaque command, so the tool name says nothing and the command has to. The
    marker is checked first: after unlock nothing here is parsed at all.
    """
    tool_input = payload.get("tool_input")
    command = tool_input.get("command") if isinstance(tool_input, dict) else None
    if not isinstance(command, str) or not command:
        append_log({**entry, "event": "native_search_allowed", "reason": "malformed_payload"})
        return ALLOW
    allow_reason = None
    if marker_path(session_id).exists():
        allow_reason = "already_unlocked"
    elif not broad_shell_search(command, _payload_cwd(payload)):
        allow_reason = "not_a_tree_search"
    elif not _marker_storage_ready():
        allow_reason = "marker_storage_unavailable"
    if allow_reason is not None:
        append_log({**entry, "event": "native_search_allowed", "reason": allow_reason})
        return ALLOW
    append_log({**entry, "event": "denied", "reason": "shell_search_before_structure"})
    sys.stderr.write(DENY_MESSAGE)
    return DENY


def main() -> int:
    # Fail open. The exit code stays ALLOW unless decide() returns one, so any
    # exception it raises leaves the call permitted for the reason the module
    # docstring gives.
    code = ALLOW
    with contextlib.suppress(Exception):
        try:
            code = decide()
        except json.JSONDecodeError:
            append_log(
                {
                    "ts": time.time(),
                    "session_id": None,
                    "event": "native_search_allowed",
                    "reason": "malformed_payload",
                }
            )
    return code


if __name__ == "__main__":
    raise SystemExit(main())
