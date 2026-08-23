"""The numeric-bound rules both front doors have to agree on.

A bound was enforced in four places that did not know about each other:
``application/symbol_service._check_limit``, two rules written by hand in
``adapters/cli/main``, one in ``core/htmlgraph``, and a ``Field(ge=, le=)`` on
every parameter of ``adapters/mcp``. Every numeric CLI argument was a bare
``type=int``, so one number meant four things depending on which command read
it: ``--limit -1`` was a usage error on ``communities``, a domain error on
``refs``, and a clean success on ``cycles``.

The rule lives here; the two doors keep their own way of reporting it. That
split is deliberate rather than a leftover. A bad value on the command line is
a usage error and exits 2 with argparse's own wording, because that is what a
person at a terminal expects; the same value through a service call is an
``AgentlessError``, because a library caller and the MCP adapter need it as a
value they can catch. The adapter converts. What neither may do is decide the
rule.

``util`` rather than ``application`` because the import contract runs
``adapters -> application -> core -> prompts -> util``, and both adapters and
every service have to reach it.
"""

from __future__ import annotations

from agentless_mcp.util.errors import AgentlessError


def at_least(value: int, minimum: int, name: str) -> int:
    """Return ``value`` when it reaches ``minimum``; refuse it by name if not.

    Refused rather than clamped. A limit of zero renders "no references to
    save_config" for a symbol with fifty-two of them -- a confident false
    negative, and the most expensive wrong answer a listing can give. A
    negative limit is worse: it slices from the end, keeps everything but the
    last row, and then quotes itself back as "the per-call limit is -1". Both
    are caller mistakes, and answering a different question silently is not a
    kindness.
    """
    if value < minimum:
        message = f"{name} must be at least {minimum}, got {value}"
        raise AgentlessError(message)
    return value


def within(value: float, low: float, high: float, name: str) -> float:
    """Return ``value`` when it falls inside ``[low, high]``; refuse it if not.

    Inclusive at both ends, because every ceiling in this package is a value
    the caller is allowed to ask for rather than one to stay under.
    """
    if not low <= value <= high:
        message = f"{name} takes a value from {low} through {high}, got {value}"
        raise AgentlessError(message)
    return value
