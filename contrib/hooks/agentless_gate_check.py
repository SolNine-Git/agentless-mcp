#!/usr/bin/env python3
"""PreToolUse hook: gate broad native search on structural localization.

This is the enforcing half of the structural-first gate. It reads the marker
that ``agentless_gate_mark.py`` writes for the calling session:

* marker present -- exit 0, and the call proceeds;
* an exact-file ``Grep`` -- exit 0 even without a marker, because the caller
  has already localized the search;
* any broader ``Grep`` or ``Glob`` without a marker -- exit 2, which blocks
  the call and returns this hook's stderr as a just-in-time instruction.

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
SEARCH_TOOLS = frozenset({"Grep", "Glob"})

DENY_MESSAGE = """Structural pass first: this session has not localized with agentless yet.

Call mcp__agentless__orient with operation="map" and focus seeds from the task \
(file paths or symbol names). Escalate with mcp__agentless__symbols \
operation="expand" on the stable ids it returns.

Broad Grep and Glob unlock after that call. Grep scoped to one existing file \
is already allowed; use broad text search afterwards for string literals, \
error messages, config keys, and test names.
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
