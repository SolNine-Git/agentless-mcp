"""Typed error hierarchy shared by every layer.

Every error carries a message an operator can act on: what was refused or
exceeded, and what to do about it. Adapters turn these into exit codes and
tool-level degradation messages; nothing catches-and-drops them.
"""


class AgentlessError(Exception):
    """Base class for every error this package raises deliberately."""


class SecurityRefusal(AgentlessError):
    """A path or argument was refused because it escapes the allowed root.

    Carries the resolved form only. Raw arguments are never echoed back:
    an error message is an output channel like any other.
    """


class WalkBoundExceeded(AgentlessError):
    """A traversal hit one of the configured bounds (depth, files, bytes)."""


class LanguageUnavailable(AgentlessError):
    """A grammar is not loadable without a network fetch, or failed to load.

    Raised instead of downloading on the tool path: fetching is a warmup-time
    decision, never a side effect of answering a query.
    """


class RepoResolutionError(AgentlessError):
    """A repository root could not be resolved or interrogated."""


class CacheLocked(AgentlessError):
    """Another process holds the tag cache's write lock.

    Raised rather than queued: an index run that waits silently behind another
    one looks like a hang, and the caller can always run it again once the
    holder finishes.
    """
