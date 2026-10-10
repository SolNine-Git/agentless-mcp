"""A per-call time limit that the long loops check, because a thread cannot be killed.

The MCP server runs each handler in a worker thread, and nothing outside a
thread can stop it, so a call its client abandoned used to run to the end. The
server now gives each call a :class:`Deadline`, and the loops that can run long
call :func:`checkpoint`. Where no deadline is active, as in the CLI and the
background index, a checkpoint does nothing.

Time spent parsing files the tag cache does not hold is credited back, up to
the grace, so a call that rebuilds a cold cache is not stopped for doing so.
"""

from __future__ import annotations

import threading
import time
from collections.abc import Iterator
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import dataclass, field

from agentless_mcp.util.errors import CallStopped

# The health complexity walk and lint's clone index stop at the lesser of this and half
# the call's remaining allowance, so a large repository cannot cost the rest of the answer.
WALK_BUDGET_SECONDS = 10.0


@dataclass
class Deadline:
    """One call's time limit, the parse time credited to it, and whether its client left."""

    limit: float
    grace: float
    started: float = field(default_factory=time.monotonic)
    credited: float = 0.0
    cancelled: threading.Event = field(default_factory=threading.Event)

    def cancel(self) -> None:
        """Mark the call abandoned, so its thread stops at the next checkpoint."""
        self.cancelled.set()

    def credit(self, seconds: float) -> None:
        """Add parse time to the call's allowance, up to the grace."""
        self.credited = min(self.grace, self.credited + seconds)

    def remaining(self) -> float:
        """Return the seconds left in the call's allowance, negative once it is spent."""
        return self.limit + self.credited - (time.monotonic() - self.started)

    def check(self) -> None:
        """Raise CallStopped when the client left or the allowance is spent."""
        if self.cancelled.is_set():
            message = "the client cancelled this call, so it stopped at its next checkpoint"
            raise CallStopped(message)
        elapsed = time.monotonic() - self.started
        if elapsed > self.limit + self.credited:
            message = (
                f"this call stopped after {elapsed:.0f} s. The server allows {self.limit:g} s "
                f"of work per call, plus up to {self.grace:g} s for parsing files the tag "
                f"cache does not hold, and this call had {self.credited:.0f} s of that grace. "
                "Narrow the request, or run `agentless-mcp index` on the repository so that "
                "later calls read a warm cache. The server flags --call-limit and "
                "--recache-grace set the two bounds."
            )
            raise CallStopped(message)


_ACTIVE: ContextVar[Deadline | None] = ContextVar("agentless_mcp_deadline", default=None)


def active() -> Deadline | None:
    """Return the deadline of the call this code runs in, or None outside one."""
    return _ACTIVE.get()


@contextmanager
def bounded(deadline: Deadline) -> Iterator[Deadline]:
    """Make ``deadline`` active inside the block and in every thread it starts."""
    token = _ACTIVE.set(deadline)
    try:
        yield deadline
    finally:
        _ACTIVE.reset(token)


def remaining() -> float | None:
    """Return the seconds left in the current call's allowance, or None outside a call."""
    deadline = _ACTIVE.get()
    return None if deadline is None else deadline.remaining()


def walk_budget(remaining: float | None) -> float:
    """Return a long walk's budget in seconds, given the call's remaining allowance."""
    if remaining is None:
        return WALK_BUDGET_SECONDS
    return max(0.0, min(WALK_BUDGET_SECONDS, remaining / 2))


def checkpoint() -> None:
    """Stop the current call if its client left or its allowance is spent."""
    deadline = _ACTIVE.get()
    if deadline is not None:
        deadline.check()


@contextmanager
def recaching() -> Iterator[None]:
    """Credit the time spent inside the block to the current call's grace."""
    deadline = _ACTIVE.get()
    started = time.monotonic()
    try:
        yield
    finally:
        if deadline is not None:
            deadline.credit(time.monotonic() - started)
