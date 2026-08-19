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

Refusals name the allowed roots in resolved form and never echo the caller's
raw argument: an error message is an output channel like any other.
"""

from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from agentless_mcp.core import gitinfo, projectconfig
from agentless_mcp.core.cache import FileSource
from agentless_mcp.core.projectconfig import ProjectConfig
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import RepoResolutionError, SecurityRefusal


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
    """

    root: Path
    head_sha: str | None
    tree_oid: str | None
    dirty_count: int | None
    note: str = ""
    symbols: FileSource | None = None
    config: ProjectConfig = projectconfig.EMPTY


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
    )


def resolved_allowlist(roots: Sequence[str | Path]) -> list[Path]:
    """Realpath every configured root once, at startup."""
    return [Path(root).expanduser().resolve() for root in roots]


def _authorise(resolved: Path, allowlist: Sequence[Path]) -> None:
    """Refuse anything that is not exactly one of the allowed roots."""
    allowed = sorted({Path(entry).expanduser().resolve() for entry in allowlist})
    if resolved in allowed:
        return

    if not allowed:
        message = MESSAGES.repo_refused_no_roots
        raise SecurityRefusal(message)

    listing = ", ".join(str(path) for path in allowed)
    message = MESSAGES.repo_refused_not_allowed.format(roots=listing)
    raise SecurityRefusal(message)
