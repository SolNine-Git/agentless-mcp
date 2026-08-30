"""Resolve and authorise the repository a call is about.

Every entry point -- one CLI invocation, one MCP tool call -- starts here, and
this is the only place a caller-supplied root becomes a path this package will
read from. Two rules make that safe:

* **Canonicalize, then check.** The raw root is realpath'd first and compared
  afterwards. Checking the textual form and resolving later is the pattern
  behind most path-traversal findings in this class of tool.
* **Exact match against a resolved allowlist.** A server started with
  ``--root /a --root /b`` serves those two directories and nothing under,
  beside or symlinked from them. Containment would be the wrong test here: a
  repository is a unit of allowlisting, and "somewhere under an allowed root"
  is how a workspace of seven repositories turns into one. The allowlist an
  adapter hands in is the operator's list: the MCP adapter may narrow it when
  a call has to pick one root, and nothing may widen it, least of all a value
  the connected client supplied.

``allowlist=None`` is CLI mode, where the root is derived from the process's
own cwd and is therefore already as trusted as the process is. That is a
different question from "which repositories may this long-lived server touch",
so it is a different value rather than an empty list.

An allowlist that is not ``None`` arrives resolved, from
:func:`resolved_allowlist` and from nowhere else. One owner for that step, and
it is the startup one: a resolution repeated per call means an entry whose
symlink target moves after startup quietly changes what the server authorises,
and an operator who wrote ``--root /srv/app`` gets whatever ``/srv/app`` points
at this second rather than what it pointed at when the server came up. An
unresolved entry now refuses a root it would once have admitted, which is the
direction a mistake in this rule has to fail.

Refusals name the allowed roots in resolved form and never echo the caller's
raw argument: an error message is an output channel like any other.
"""

import sqlite3
from collections.abc import Sequence
from dataclasses import dataclass, field
from pathlib import Path

from agentless_mcp.core import gitinfo, projectconfig
from agentless_mcp.core.cache import FileSource
from agentless_mcp.core.projectconfig import ProjectConfig
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import RepoResolutionError, SecurityRefusal

# What the receipt says when a call carries no symbol source at all: nothing
# was cached, so the answer was parsed on demand.
CACHE_NONE = "none"

# What reading a symbol source's own description may fail with without taking
# the answer down with it. The tag cache reaches SQLite to describe itself, and
# a locked, truncated or vanished database is a fact about the receipt rather
# than about the symbols the caller asked for. Named explicitly, the way
# ``gitinfo`` names its three, rather than catching ``Exception``: anything
# else raised here is a defect and must surface as one.
_RECEIPT_FAILURES: tuple[type[Exception], ...] = (sqlite3.DatabaseError, OSError)


@dataclass(frozen=True)
class RepoContext:
    """The repository one call is about, with the git state it was read at.

    Snapshotted once per call: every view rendered from this context reports
    the same HEAD and dirty count, so a receipt describes the state the answer
    was actually computed from rather than the state at the moment it was
    printed.

    ``symbols`` is where this call's views get their symbols from -- the tag
    cache when one is open, on-demand parsing otherwise. It rides on the
    context rather than on each service because it is per-call repository
    state exactly like ``head_sha``, and because a service that never learns
    which of the two it is holding cannot answer differently depending on the
    answer. ``None`` is the index-free default: no cache was opened at all.

    ``cache_receipt`` is the ``cache:`` field of the response receipt,
    snapshotted here beside the git state and for the same reason. Read from
    the source on every render instead, it cost four ``COUNT(*)`` scans of the
    index per render -- twice over for a CLI call that prints a stderr receipt
    beside a JSON body -- to produce a string whose numbers the receipt does
    not use.
    """

    root: Path
    head_sha: str | None
    tree_oid: str | None
    dirty_count: int | None
    note: str = ""
    symbols: FileSource | None = None
    config: ProjectConfig = projectconfig.EMPTY
    # The lazy churn lookup the map header spends, set only when git answered
    # for the root. None for a root outside git and for any hand-built
    # context -- the characterization goldens pin their git state through
    # exactly that door, so churn enters through the same boundary as
    # ``head_sha`` and a pinned context stays hermetic by construction.
    churn: gitinfo.ChurnSource | None = None
    cache_receipt: str = field(init=False, default=CACHE_NONE)

    def __post_init__(self) -> None:
        """Snapshot the symbol source's own description, once."""
        object.__setattr__(self, "cache_receipt", _cache_receipt(self.symbols))

    def close(self) -> None:
        """Release request-scoped repository resources."""
        if self.symbols is not None:
            self.symbols.close()


def resolve_repo(raw_root: str | Path, allowlist: Sequence[Path] | None) -> RepoContext:
    """Resolve ``raw_root``, authorise it, and snapshot its state.

    The project config is read here, with the git snapshot, because both are
    facts about the repository at the moment the call started and both belong
    to every view the call renders. Reading it cannot fail the call: an absent,
    malformed or oversized file produces an empty configuration whose warnings
    ride in the response envelope.
    """
    resolved = Path(raw_root).expanduser().resolve()

    if allowlist is not None:
        _authorise(resolved, allowlist)

    if not resolved.is_dir():
        message = f"not a directory: {resolved}"
        raise RepoResolutionError(message)

    snapshot = gitinfo.snapshot(resolved)
    return RepoContext(
        root=resolved,
        head_sha=snapshot.head_sha,
        tree_oid=snapshot.tree_oid,
        dirty_count=snapshot.dirty_count,
        note=snapshot.note,
        config=projectconfig.load(resolved),
        churn=gitinfo.ChurnSource(resolved) if snapshot.head_sha else None,
    )


def _cache_receipt(source: FileSource | None) -> str:
    """Describe one call's symbol source, degrading a failure into a note.

    The git half of this receipt is built never to raise, on the stated ground
    that a courtesy that can hang is a bug. The cache half reaches SQLite to
    count its rows, so it can raise on a locked or truncated index -- and take
    with it an answer that never depended on those counts. One receipt, one
    failure contract: the field says it is unavailable and the call proceeds.
    """
    if source is None:
        return CACHE_NONE
    try:
        return source.receipt
    except _RECEIPT_FAILURES as exc:
        return f"{CACHE_NONE} (cache status unavailable: {exc})"


def resolved_allowlist(roots: Sequence[str | Path]) -> list[Path]:
    """Realpath every configured root once, at startup.

    The one owner of that resolution. :func:`_authorise` compares what this
    returned and resolves nothing of its own.
    """
    return [Path(root).expanduser().resolve() for root in roots]


def _authorise(resolved: Path, allowlist: Sequence[Path]) -> None:
    """Refuse anything that is not exactly one of the allowed roots.

    ``allowlist`` is trusted to hold resolved paths, because
    :func:`resolved_allowlist` produced it at startup.
    """
    allowed = sorted(set(allowlist))
    if resolved in allowed:
        return

    if not allowed:
        message = MESSAGES.repo_refused_no_roots
        raise SecurityRefusal(message)

    listing = ", ".join(str(path) for path in allowed)
    message = MESSAGES.repo_refused_not_allowed.format(roots=listing)
    raise SecurityRefusal(message)
