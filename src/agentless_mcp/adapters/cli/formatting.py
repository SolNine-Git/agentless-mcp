"""Output and exit-code conventions shared by every CLI subcommand.

The split is deliberate and load-bearing for an agent driving this over Bash:
**stdout carries the answer, stderr carries everything about the run**. A
pipeline that captures stdout gets only the receipt, the banner and the view;
a failure is on stderr with a non-zero exit, never interleaved into a view
that would then parse as a shorter answer.

Exit codes are the same three the plan fixes, and this is the whole contract
-- every subcommand keys on the same distinction rather than deciding it
locally:

* ``0`` -- the call answered. An answer that is *legitimately* empty counts:
  a repository with no import cycles, no communities, no callers of a symbol
  that exists. A patch that applied is an answer.
* ``1`` -- a domain failure. The request made sense; the repository did not
  allow it. Nothing the caller named resolved (no such symbol, no such file,
  an id matching nothing), a grammar that will not load, a walk bound
  exceeded, a patch that did not parse or did not apply, a diagram that has
  drifted, a validation nothing survived, a ranking with no tier.
* ``2`` -- usage or security. The request itself was not admissible: a bad
  flag or a bound out of range, an input file that could not be read, a
  missing repository root, a path or root refused.

The line between ``0`` and ``1`` is *"did what the caller named exist"*, not
*"is the answer empty"*: ``skeleton nope.py`` and ``slice nope.py`` are the
same failure and carry the same code.
"""

import sys

from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.util.errors import (
    AgentlessError,
    InputUnreadable,
    RepoResolutionError,
    SecurityRefusal,
)
from agentless_mcp.util.textsafe import one_line

EXIT_OK = 0
EXIT_DOMAIN = 1
EXIT_USAGE = 2


def emit(text: str) -> None:
    """Write an answer to stdout, exactly once, newline-terminated."""
    sys.stdout.write(text if text.endswith("\n") else text + "\n")


def note(text: str) -> None:
    """Write a receipt or a summary to stderr, leaving stdout to the answer.

    The write subcommands emit a unified diff or an equivalence key on stdout,
    which a caller pipes straight into ``git apply`` or a comparison. Anything
    describing the run -- which repository, which HEAD, how many edits landed
    -- goes here, so that pipeline never has to be told to skip a header.
    """
    sys.stderr.write(text if text.endswith("\n") else text + "\n")


def fail(message: str, code: int = EXIT_DOMAIN) -> int:
    """Report a failure on stderr and return the exit code to propagate.

    One line, always: a refusal quotes what the caller named and what the
    repository holds -- an ambiguous endpoint is answered with the stable ids
    it matched -- so the message is repository text and this is the sink that
    places it on a line. An agent driving this over Bash reads stderr the way
    it reads stdout, and a refusal that spans three lines is three refusals to
    whatever splits it.
    """
    sys.stderr.write(f"agentless-mcp: {one_line(message)}\n")
    return code


def exit_code_for(error: AgentlessError) -> int:
    """Map a typed error onto its exit code.

    Keyed on what the error *is* -- a refusal versus a degraded answer -- not
    on which subcommand raised it, so a new subcommand inherits the mapping
    instead of re-deciding it. A refused path or root, and an input file the
    caller named that could not be read, all say the request was not
    admissible; everything else the repository refused to answer is a domain
    failure, including ``LanguageUnavailable`` and ``WalkBoundExceeded``,
    which is why they need no branch of their own.

    ``InputUnreadable`` is here rather than matched on a message because the
    subcommands that read a caller's file through a service and the ones that
    read it inline have to agree: ``lint --candidates gone`` and ``vote
    --verdicts gone.jsonl`` are the same mistake and now carry the same code.
    """
    if isinstance(error, SecurityRefusal | RepoResolutionError | InputUnreadable):
        return EXIT_USAGE
    return EXIT_DOMAIN


def warn_about(ctx: RepoContext) -> None:
    """Put a degraded repository state on stderr as well as in the receipt.

    The receipt already carries the note, but a caller that pipes stdout into
    a prompt would never see it there in time to act.

    The same value the receipt renders, so it gets the same escape the receipt
    applies to it: ``application.envelope`` puts ``ctx.note`` through
    :func:`one_line` on the receipt line, and one value escaped at one of its
    two sinks is the asymmetry that makes a rule impossible to state.
    """
    if ctx.note:
        sys.stderr.write(f"agentless-mcp: {one_line(ctx.note)}\n")
