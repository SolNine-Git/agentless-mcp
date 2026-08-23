"""Bounded, gitignore-aware directory tree rendering.

Two steps kept apart on purpose: :func:`walk_repo` decides which files exist
as far as this tool is concerned, and :func:`render_tree` decides how many of
them a caller is shown. A view that silently drops entries is worse than a
short one, so every truncation is marked in the output.

Inside a git repository the file list comes from git itself
(``git ls-files --cached --others --exclude-standard``), which is the only
way to honour nested ``.gitignore`` files, ``.git/info/exclude`` and the
user's global excludes without reimplementing them. Outside one, the bounded
walk is the fallback. Either way the security bounds in
:mod:`agentless_mcp.util.fslimits` still apply.
"""

import subprocess
from dataclasses import dataclass
from pathlib import Path

from agentless_mcp.core import gitinfo
from agentless_mcp.util.errors import RepoResolutionError, WalkBoundExceeded
from agentless_mcp.util.fslimits import (
    DEFAULT_MAX_BYTES,
    DEFAULT_MAX_DEPTH,
    DEFAULT_MAX_WALK_FILES,
    bounded_walk,
    file_stays_inside,
)

DEFAULT_RENDER_DEPTH = 4
DEFAULT_MAX_ENTRIES = 500

# git is out-of-process: it gets a timeout like every other external call.
GIT_TIMEOUT_SECONDS = 30


@dataclass(frozen=True)
class RepoFile:
    """One file in the repository, relative to the root that produced it."""

    path: str
    size: int

    @property
    def parts(self) -> tuple[str, ...]:
        """The path split into components.

        A forward-slash split on every platform, and correct on every
        platform: this path is always repository-relative in posix form --
        either ``as_posix()`` of a resolved relative path, or a line of
        ``git ls-files``, which emits forward slashes on Windows too. The
        native separator never reaches this field.
        """
        return tuple(self.path.split("/"))


def is_git_repo(root: Path) -> bool:
    """True when ``root`` lies inside a git work tree.

    Asked of git rather than of the filesystem: a ``.git`` entry at exactly
    this path is a correlate, and it is absent for every root git still
    answers for -- a package inside a monorepo, a linked worktree, a
    submodule. ``git -C <root> ls-files`` scopes its output to the given
    directory, so the branch below stays correct wherever the root sits.
    """
    return gitinfo.git_root(root) is not None


def walk_repo(
    root: Path,
    *,
    max_depth: int = DEFAULT_MAX_DEPTH,
    max_files: int = DEFAULT_MAX_WALK_FILES,
    max_bytes: int = DEFAULT_MAX_BYTES,
) -> list[RepoFile]:
    """List every file under ``root`` that git would not ignore.

    Sorted by path so two calls on an unchanged tree render identically.
    Raises :class:`WalkBoundExceeded` rather than truncating; raises
    :class:`RepoResolutionError` when the root is not a directory or git
    refuses to answer.
    """
    resolved = root.resolve()
    if not resolved.is_dir():
        message = f"not a directory: {resolved}"
        raise RepoResolutionError(message)

    if is_git_repo(resolved):
        relatives = _git_listed_paths(resolved)
    else:
        relatives = [
            path.relative_to(resolved).as_posix()
            for path in bounded_walk(
                resolved, max_depth=max_depth, max_files=max_files, max_bytes=max_bytes
            )
        ]

    files: list[RepoFile] = []
    total_bytes = 0
    for relative in sorted(set(relatives)):
        candidate = resolved / relative
        if not file_stays_inside(candidate, resolved):
            # git lists index entries for files deleted in the working tree,
            # and it lists a tracked symlink whatever it points at. The same
            # containment rule the bounded walk applies covers both: a link
            # out of the tree is no more servable here than one found by the
            # fallback traversal.
            continue
        depth = len(Path(relative).parts) - 1
        if depth > max_depth:
            message = (
                f"walk refused: path depth {depth} exceeds the limit of {max_depth} "
                f"under {resolved}; point the call at a subdirectory instead"
            )
            raise WalkBoundExceeded(message)

        size = candidate.stat().st_size
        total_bytes += size
        if len(files) + 1 > max_files:
            message = (
                f"walk refused: more than {max_files} files under {resolved}; "
                f"point the call at a subdirectory or raise the file bound"
            )
            raise WalkBoundExceeded(message)
        if total_bytes > max_bytes:
            message = (
                f"walk refused: more than {max_bytes} bytes under {resolved}; "
                f"point the call at a subdirectory or raise the byte bound"
            )
            raise WalkBoundExceeded(message)

        files.append(RepoFile(path=relative, size=size))

    return files


def _git_listed_paths(root: Path) -> list[str]:
    """Return tracked plus untracked-not-ignored paths, via git."""
    command = [
        "git",
        *gitinfo.HARDENING_PREFIX,
        "-C",
        str(root),
        "ls-files",
        "--cached",
        "--others",
        "--exclude-standard",
        "-z",
    ]
    try:
        # Fixed argv, no shell, and nothing from repository content reaches it.
        completed = subprocess.run(
            command,
            capture_output=True,
            timeout=GIT_TIMEOUT_SECONDS,
            check=False,
        )
    except FileNotFoundError as exc:
        message = f"git is not installed, so {root} cannot be listed the way its .gitignore asks"
        raise RepoResolutionError(message) from exc
    except subprocess.TimeoutExpired as exc:
        message = f"git ls-files timed out after {GIT_TIMEOUT_SECONDS}s in {root}"
        raise RepoResolutionError(message) from exc

    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        message = f"git ls-files failed in {root} (exit {completed.returncode}): {detail}"
        raise RepoResolutionError(message)

    stdout = completed.stdout.decode("utf-8", errors="replace")
    return [entry for entry in stdout.split("\0") if entry]


def render_tree(
    files: list[RepoFile],
    depth: int = DEFAULT_RENDER_DEPTH,
    max_entries: int = DEFAULT_MAX_ENTRIES,
) -> str:
    """Render an indented tree, four spaces per level, directories with a slash.

    Entries below ``depth`` are elided with a ``...`` marker under their
    parent, and the whole render stops after ``max_entries`` names with a
    trailing marker naming how many were left out. Both markers exist so a
    reader never mistakes a bounded view for the whole repository.
    """
    tree = _build_tree(files)
    lines: list[str] = []
    budget = _Budget(remaining=max_entries, omitted=0)
    _render_level(tree, level=0, depth=depth, budget=budget, lines=lines)

    if budget.omitted:
        lines.append(f"... {budget.omitted} more entries truncated (max_entries={max_entries})")

    return "\n".join(lines) + "\n" if lines else ""


@dataclass
class _Budget:
    """How many more names may be rendered, and how many were left out."""

    remaining: int
    omitted: int


_Tree = dict[str, "_Tree | None"]


def _build_tree(files: list[RepoFile]) -> _Tree:
    """Build a nested mapping; a file is a key mapped to None."""
    tree: _Tree = {}
    for repo_file in files:
        cursor = tree
        parts = repo_file.parts
        for part in parts[:-1]:
            child = cursor.get(part)
            if not isinstance(child, dict):
                child = {}
                cursor[part] = child
            cursor = child
        cursor.setdefault(parts[-1], None)
    return tree


def _render_level(
    tree: _Tree,
    level: int,
    depth: int,
    budget: _Budget,
    lines: list[str],
) -> None:
    """Append one level of the tree, recursing while depth allows."""
    indent = " " * (4 * level)
    for name in sorted(tree):
        child = tree[name]
        if budget.remaining <= 0:
            budget.omitted += _count_entries(child) + 1
            continue

        budget.remaining -= 1
        if child is None:
            lines.append(f"{indent}{name}")
            continue

        lines.append(f"{indent}{name}/")
        if level + 1 >= depth:
            # Depth elision is marked where it happens; only entries dropped
            # for the entry budget feed the trailing count, so the two
            # truncations never double-report the same files.
            if _count_entries(child):
                lines.append(f"{indent}    ...")
            continue
        _render_level(child, level + 1, depth, budget, lines)


def _count_entries(node: "_Tree | None") -> int:
    """Count the names under a node, itself excluded."""
    if node is None:
        return 0
    return sum(1 + _count_entries(child) for child in node.values())
