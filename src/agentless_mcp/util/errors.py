"""Typed error hierarchy shared by every layer.

Every error carries a message an operator can act on: what was refused or
exceeded, and what to do about it. Adapters turn these into exit codes and
tool-level degradation messages; nothing catches-and-drops them.
"""


class AgentlessError(Exception):
    """The taxonomy root: catch this to catch every error this package raises.

    A base and nothing else. Raised directly it would mean two things at
    once -- "any error from this package" to the handler, and "a condition
    nobody bothered to classify" at the raise site -- and no handler can
    tell those apart. Every raise names a leaf instead; the leaf for a
    condition with no class of its own is :class:`OperationFailed`.
    """


class OperationFailed(AgentlessError):
    """The operation could not be carried out as the caller asked for it.

    The leaf for a condition worth no distinction of its own: the request
    was understood, and the repository, the input or the run did not allow
    it. Adapters map it to a domain failure. A condition a caller would act
    on differently gets its own class here rather than this one.
    """


class InputUnreadable(AgentlessError):
    """A file the caller named could not be read.

    Its own class because the argument is wrong rather than the repository:
    the path is absent, names a directory where a file was wanted, or holds
    bytes no UTF-8 decoder accepts. That is a usage failure, and the CLI's
    own readers already reported it as one -- this is how the services that
    load a caller's file reach the same exit code instead of a different
    one for the same mistake.
    """


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
