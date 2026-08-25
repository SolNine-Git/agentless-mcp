#!/usr/bin/env python3
"""PostToolUse hook: record meaningful structural localization.

This is the observing half of the structural-first gate. It writes a marker
after a call that proves the caller knows where to look. Diagnostics and
directory listings do not qualify, so calling ``capabilities`` cannot unlock
broad native search. The marker filename is a digest of the session id, so one
session never unlocks another and untrusted ids never become path components.

Every decision is logged with the ``operation`` argument, not only the tool
name. The gate treats ``read(slice)`` and ``read(dir)`` differently, and a log
that recorded neither could not tell afterwards which of them a session was
refused.

The hook reads the call, not its result. ``tool_response`` has no shape this
hook can read a success out of across every tool it matches, and guessing one
would silently stop unlocking the moment that shape changed. An Agentless call
that errored still unlocks, which costs one premature unlock and never a
session wedged shut.

The hook always exits 0. A PostToolUse hook that fails shows up as a tool
error on a call that already succeeded, so every failure path here -- malformed
stdin, an unwritable ``/tmp``, a missing session id -- exits 0 and leaves the
session alone. The cost of failing open is one denied ``Grep`` that the model
repeats after its next structural call. The cost of failing closed is a broken
session that never gets its text search back.

Set ``AGENTLESS_GATE_LOG`` to a file path to also append one JSONL line per
call. The variable is optional, and it is unset by default. Without the log, a
session that finished proves only that it finished, not that the gate fired.

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

# What unlocks is a call that proves the caller knows where to look.
#
# `orient(path)` qualifies for the same reason `symbols(find)` does: it returns
# resolved relationships between two named symbols, which is stronger evidence
# than a substring name lookup. `read(slice)` qualifies on the rule the check
# hook already applies to an exact-file Grep -- naming a file and a line range
# IS the localization, and refusing it while allowing the weaker Grep was a
# contradiction between the two halves of one gate.
#
# `read(dir)` stays out: a directory listing is how you look for a file, not
# evidence that you found one. So do the listings that answer "how is this
# repository shaped" -- communities, cycles, diagram, health.
LOCALIZING_OPERATIONS = {
    "mcp__agentless__orient": frozenset({"map", "path"}),
    "mcp__agentless__symbols": frozenset({"find", "overview", "expand", "explain"}),
    "mcp__agentless__read": frozenset({"slice"}),
}
LOCALIZING_TOOLS = frozenset(
    {
        "mcp__agentless__repo_map",
        "mcp__agentless__get_symbols_overview",
        "mcp__agentless__expand_symbols",
        "mcp__agentless__find_symbol",
        "mcp__agentless__explain_symbol",
        # The v1 spelling of `read(slice)`; v1's `list_dir` is `read(dir)` and
        # is deliberately absent.
        "mcp__agentless__read_slice",
    }
)
REFERENCE_TOOL = "mcp__agentless__find_referencing_symbols"


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


def operation_of(payload: dict[str, object]) -> str | None:
    """Return the grouped tool's ``operation`` argument, or None.

    Logged on every decision, unlocking or not. Without it a refusal records
    only that some ``read`` was ignored, and ``read(slice)`` and ``read(dir)``
    -- which this gate now treats differently -- are indistinguishable after
    the fact. A measurement run cannot recover what the log did not keep.
    """
    tool_input = payload.get("tool_input")
    if not isinstance(tool_input, dict):
        return None
    operation = tool_input.get("operation")
    return operation if isinstance(operation, str) else None


def unlock_reason(payload: dict[str, object]) -> str | None:
    """Name the localizing operation, or return none for a non-localizing call."""
    tool = payload.get("tool_name")
    if tool == REFERENCE_TOOL:
        return "find_referencing_symbols"
    if tool in LOCALIZING_TOOLS:
        return str(tool).rsplit("__", 1)[-1]
    operations = LOCALIZING_OPERATIONS.get(tool)
    operation = operation_of(payload)
    if operations is not None and operation in operations:
        return f"{tool.rsplit('__', 1)[-1]}.{operation}"
    return None


def mark() -> None:
    """Write this session's marker after a localizing structural call."""
    payload = json.loads(sys.stdin.read() or "{}")
    session_id = payload.get("session_id")
    if not isinstance(session_id, str) or not session_id:
        return
    reason = unlock_reason(payload)
    if reason is None:
        append_log(
            {
                "ts": time.time(),
                "session_id": session_id,
                "event": "agentless_call_ignored",
                "tool": payload.get("tool_name"),
                "operation": operation_of(payload),
                "reason": "non_localizing_operation",
            }
        )
        return
    MARKER_DIR.mkdir(parents=True, exist_ok=True)
    marker_path(session_id).write_text(
        json.dumps(
            {
                "version": 2,
                "reason": reason,
                "tool": payload.get("tool_name"),
            }
        ),
        encoding="utf-8",
    )
    append_log(
        {
            "ts": time.time(),
            "session_id": session_id,
            "event": "structural_call",
            "tool": payload.get("tool_name"),
            "operation": operation_of(payload),
            "reason": reason,
        }
    )


def main() -> int:
    # Fail open. Every exception path exits 0 for the reason the module
    # docstring gives: this hook runs after a tool call that already succeeded.
    with contextlib.suppress(Exception):
        mark()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
