"""The numeric-bound rules both front doors have to agree on.

A bound was enforced in four places that did not know about each other:
``application/symbol_service._check_limit``, two rules written by hand in
``adapters/cli/main``, one in ``core/htmlgraph``, and a ``Field(ge=, le=)`` on
every parameter of ``adapters/mcp``. Every numeric CLI argument was a bare
``type=int``, so one number meant four things depending on which command read
it: ``--limit -1`` was a usage error on ``communities``, a domain error on
``refs``, and a clean success on ``cycles``.

The rule lives here and raises; the two doors differ only in how they render
the raise. On the command line an ``OperationFailed`` reaches
``exit_code_for`` and becomes exit 1, the same code as any other answer the
repository would not give; argparse's own exit 2 is reserved for an argument
that is not a number at all, which never reaches this module. Over MCP the
same exception is converted by the adapter into a tool error the caller can
read. What neither door may do is decide the rule.

``util`` rather than ``application`` because the import contract runs
``adapters -> application -> core -> prompts -> util``, and both adapters and
every service have to reach it.
"""

from __future__ import annotations

from agentless_mcp.util.errors import OperationFailed

# The ceilings, held here rather than in the MCP adapter that publishes them.
# A ceiling enforced only on the wire is a ceiling the CLI does not have:
# ``cycles --limit 100000`` was answered and ``orient(operation="cycles",
# limit=100000)`` refused, for the same repository on the same call. The
# adapter still advertises these in its JSON schema -- that schema is the only
# refusal a model can read before it calls -- but it reads the numbers from
# here instead of owning them, so the two doors cannot drift apart.
#
# The tree bounds are not here: ``depth`` and ``max_entries`` are capped at
# the traversal limits in :mod:`agentless_mcp.util.fslimits`, which already
# owns them and which this module must not import, because fslimits imports
# this one.
MAX_LIMIT = 500
MAX_CONTEXT_LINES = 200
MAX_DIAGRAM_NODES = 500
MAX_DIAGRAM_EDGES = 500
MAX_RESOLUTION = 100.0


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
        raise OperationFailed(message)
    return value


def within(value: float, low: float, high: float, name: str) -> float:
    """Return ``value`` when it falls inside ``[low, high]``; refuse it if not.

    Inclusive at both ends, because every ceiling in this package is a value
    the caller is allowed to ask for rather than one to stay under.
    """
    if not low <= value <= high:
        message = f"{name} takes a value from {low} through {high}, got {value}"
        raise OperationFailed(message)
    return value
