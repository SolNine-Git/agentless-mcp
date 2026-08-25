#!/usr/bin/env python3
"""PreToolUse hook: deny Grep and Glob until one structural call has happened.

This is the enforcing half of the structural-first gate. It reads the marker
that ``agentless_gate_mark.py`` writes for the calling session:

* marker present -- exit 0, and the call proceeds. Text search is deferred,
  never withheld, so ``Grep`` keeps the one job a symbol map cannot do: string
  literals, error messages, config keys, and fixtures.
* marker absent -- exit 2, which blocks the call and returns this hook's
  stderr to the model as a just-in-time instruction.

The gate fires once per session. After the first ``mcp__agentless__*`` call,
every later ``Grep`` and ``Glob`` runs unchanged. The constraint is an order,
not a ban.

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
import json
import os
import sys
import time
from pathlib import Path

# Session state, not user data: the marker holds no content, and it is meant to
# disappear when the machine reboots. Keep this path identical in both hooks.
MARKER_DIR = Path("/tmp/agentless_gate")
LOG_ENV = "AGENTLESS_GATE_LOG"

ALLOW = 0
DENY = 2

DENY_MESSAGE = """Structural pass first: this session has not called an agentless tool yet.

Call mcp__agentless__orient with operation="map" and focus seeds from the task \
(file paths or symbol names). Escalate with mcp__agentless__symbols \
operation="expand" on the stable ids it returns.

Grep and Glob unlock after that call. Use them for what a symbol map cannot \
rank: string literals, error messages, config keys, test names.
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


def decide() -> int:
    """Return the exit code for one PreToolUse payload read from stdin."""
    payload = json.loads(sys.stdin.read() or "{}")
    session_id = payload.get("session_id")
    entry: dict[str, object] = {
        "ts": time.time(),
        "session_id": session_id,
        "tool": payload.get("tool_name"),
        "pattern": (payload.get("tool_input") or {}).get("pattern"),
    }
    # No session id means no marker can ever be written for this call, so a
    # denial here would deny every later call too. Fail open and say so.
    if not session_id:
        append_log({**entry, "event": "native_search_allowed", "reason": "no_session_id"})
        return ALLOW
    if (MARKER_DIR / f"{session_id}.ok").exists():
        append_log({**entry, "event": "native_search_allowed"})
        return ALLOW
    append_log({**entry, "event": "denied"})
    sys.stderr.write(DENY_MESSAGE)
    return DENY


def main() -> int:
    # Fail open. The exit code stays ALLOW unless decide() returns one, so any
    # exception it raises leaves the call permitted for the reason the module
    # docstring gives.
    code = ALLOW
    with contextlib.suppress(Exception):
        code = decide()
    return code


if __name__ == "__main__":
    raise SystemExit(main())
