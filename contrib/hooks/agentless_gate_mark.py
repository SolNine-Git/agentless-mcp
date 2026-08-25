#!/usr/bin/env python3
"""PostToolUse hook: record that a session made one structural call.

This is the observing half of the structural-first gate. It touches
``/tmp/agentless_gate/<session_id>.ok`` after any ``mcp__agentless__*`` tool
call. That marker file is the unlock signal ``agentless_gate_check.py`` reads.
The marker name carries the session id, so one session never unlocks another
and two concurrent sessions stay independent.

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
import json
import os
import sys
import time
from pathlib import Path

# Session state, not user data: the marker holds no content, and it is meant to
# disappear when the machine reboots. Keep this path identical in both hooks.
MARKER_DIR = Path("/tmp/agentless_gate")
LOG_ENV = "AGENTLESS_GATE_LOG"


def append_log(entry: dict[str, object]) -> None:
    """Append *entry* to ``AGENTLESS_GATE_LOG``. Do nothing when the var is unset."""
    path = os.environ.get(LOG_ENV)
    if not path:
        return
    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    with target.open("a") as handle:
        handle.write(json.dumps(entry) + "\n")


def mark() -> None:
    """Touch this session's marker file and log the call."""
    payload = json.loads(sys.stdin.read() or "{}")
    session_id = payload.get("session_id")
    if not session_id:
        return
    MARKER_DIR.mkdir(parents=True, exist_ok=True)
    (MARKER_DIR / f"{session_id}.ok").touch()
    append_log(
        {
            "ts": time.time(),
            "session_id": session_id,
            "event": "structural_call",
            "tool": payload.get("tool_name"),
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
