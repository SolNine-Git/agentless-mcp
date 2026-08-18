"""Worktree lifecycle: apply a candidate patch without touching the checkout.

The read surface of this package is strictly read-only toward an analysed
repository, and the write surface keeps that posture by default: applying a
patch materialises a detached ``git worktree`` under the user's cache
directory, writes there, and produces the diff from there. The caller's
checkout -- including any work in progress in it -- is never opened for
writing, which the tests assert by comparing ``git status --porcelain`` and
``HEAD`` before and after.

Scratch worktrees live under ``$XDG_CACHE_HOME/agentless-mcp/worktrees``,
never inside the target repository. A scratch directory inside the tree under
analysis would show up in that tree's own status, its own walk and its own
gitignore decisions, which is the same class of mistake as writing a cache
into somebody else's repo.

Removal happens in a ``finally``, so a patch that raises mid-apply still
leaves nothing behind. Cleanup failures are logged with the path rather than
raised: raising from the ``finally`` would replace whatever the caller was
already failing with, and losing the real error to report a leftover directory
is the wrong trade. The log line names the directory an operator can delete.

Every git call is a fixed argv with a timeout and no shell. Nothing derived
from repository content or from patch text reaches it.
"""

import logging
import shutil
import subprocess
import uuid
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from pathlib import Path

from agentless_mcp.core import cache, gitinfo
from agentless_mcp.util.errors import AtlasError, RepoResolutionError

logger = logging.getLogger(__name__)

# Creating a worktree copies HEAD out; on a large repository that is slower
# than a status read but nowhere near a minute. A bound that can be hit by a
# healthy repository is a flake, and no bound at all is a hang.
GIT_TIMEOUT_SECONDS = 120.0

WORKTREE_DIR = "worktrees"


def scratch_root() -> Path:
    """Return the directory scratch worktrees are created under."""
    return cache.cache_root() / WORKTREE_DIR


@contextmanager
def worktree(root: Path) -> Iterator[Path]:
    """Materialise a detached worktree of ``root`` at HEAD, and remove it after.

    Yields the directory inside the worktree that *corresponds to* ``root``.
    Git only ever creates a worktree of a whole repository, so when ``root`` is
    a subdirectory -- a package inside a monorepo, a fixture directory inside
    a larger checkout -- the yielded path is that same subdirectory within the
    copy. A caller resolving a repository-relative path against it therefore
    lands on the same file it would have in the original, which is the whole
    point of handing back a path rather than a repository.

    Refuses a directory that is not inside a git repository: without HEAD
    there is nothing to detach from, and silently falling back to copying the
    working tree would produce a diff against a base nobody named.
    """
    resolved = root.resolve()
    top = gitinfo.git_root(resolved)
    if top is None:
        message = (
            f"{resolved} is not inside a git repository, so no worktree can be created for it. "
            "Patch apply needs a git repository; run it against one, or use --in-place."
        )
        raise RepoResolutionError(message)

    scratch = scratch_root()
    scratch.mkdir(parents=True, exist_ok=True)
    scratch.chmod(cache.DIRECTORY_MODE)

    path = scratch / f"wt-{uuid.uuid4().hex}"
    run_git(top, ["worktree", "add", "--detach", str(path), "HEAD"])
    try:
        yield path / resolved.relative_to(top)
    finally:
        _release(top, path)


def diff(worktree_path: Path) -> str:
    """Return the unified diff of the uncommitted changes in ``worktree_path``.

    ``--no-color`` and ``--no-ext-diff`` are explicit because a worktree reads
    the repository's own configuration: a ``color.diff = always`` or a
    ``diff.external`` in the user's config would otherwise decide the format
    of a diff this tool promises is machine-readable.
    """
    return run_git(worktree_path, ["diff", "--no-color", "--no-ext-diff"])


def run_git(cwd: Path, arguments: Sequence[str]) -> str:
    """Run one bounded git command, raising on anything but success.

    Unlike :func:`agentless_mcp.core.gitinfo._run`, a failure here is an
    error, not a note: that module answers "what state is this repository in",
    where unknown is a legitimate answer, and this one performs the write-side
    operations where a failure means the operation did not happen.
    """
    subcommand = arguments[0] if arguments else "git"
    command = ["git", "--no-optional-locks", "-C", str(cwd), *arguments]
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            timeout=GIT_TIMEOUT_SECONDS,
            check=False,
        )
    except FileNotFoundError as exc:
        message = "git is not installed, so the patch machinery cannot run"
        raise AtlasError(message) from exc
    except subprocess.TimeoutExpired as exc:
        message = f"git {subcommand} timed out after {GIT_TIMEOUT_SECONDS}s in {cwd}"
        raise AtlasError(message) from exc
    except OSError as exc:
        message = f"git {subcommand} could not be run in {cwd}: {exc.strerror}"
        raise AtlasError(message) from exc

    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        first = detail.splitlines()[0] if detail else "no detail"
        message = f"git {subcommand} exited {completed.returncode} in {cwd}: {first}"
        raise AtlasError(message)

    return completed.stdout.decode("utf-8", errors="replace")


def _release(root: Path, path: Path) -> None:
    """Remove one scratch worktree and prune the repository's record of it."""
    try:
        run_git(root, ["worktree", "remove", "--force", str(path)])
    except AtlasError as exc:
        logger.warning("git worktree remove failed for %s: %s; removing the directory", path, exc)
        shutil.rmtree(path, ignore_errors=True)

    try:
        run_git(root, ["worktree", "prune"])
    except AtlasError as exc:
        logger.warning("git worktree prune failed in %s: %s", root, exc)

    if path.exists():
        logger.error(
            "scratch worktree %s could not be removed and is still on disk; delete it by hand",
            path,
        )
