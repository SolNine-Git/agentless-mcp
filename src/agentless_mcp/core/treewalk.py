"""Bounded, gitignore-aware directory tree rendering.

Two steps kept apart on purpose: :func:`walk_repo` decides which files exist
as far as this tool is concerned, and :func:`render_tree` decides how many of
them a caller is shown. A view that silently drops entries is worse than a
short one, so every truncation is marked in the output.

Inside a git repository that owns the root, the file list comes from git
itself (``git ls-files --cached --others --exclude-standard``), which is the
only way to honour nested ``.gitignore`` files, ``.git/info/exclude`` and the
user's global excludes without reimplementing them. Outside one, the bounded
walk is the fallback. A root an enclosing work tree ignores wholesale takes
that same fallback: git answers for it and lists nothing, which is a correct
statement about that repository's contents and a false one about the tree
being served. Either way the security bounds in
:mod:`agentless_mcp.util.fslimits` still apply.
"""

import logging
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
from agentless_mcp.util.textsafe import one_line

logger = logging.getLogger(__name__)

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


def _git_listing_speaks_for(root: Path) -> bool:
    """True when a git listing describes the whole tree under ``root``.

    Asked of git rather than of the filesystem: a ``.git`` entry at exactly
    this path is a correlate, and it is absent for every root git still
    answers for -- a package inside a monorepo, a linked worktree, a
    submodule. ``git -C <root> ls-files`` scopes its output to the given
    directory, so the branch below stays correct wherever the root sits.

    That some repository answers is not enough, which is the defect this
    predicate replaces. A root the enclosing work tree ignores wholesale --
    a snapshot unpacked under a gitignored ``repos/`` directory, carrying no
    ``.git`` of its own -- makes ``ls-files --cached --others
    --exclude-standard`` return zero paths, and the tool reported an empty
    repository for a tree full of files. The invariant the branch keys on is
    that the answering repository owns the root, not that a repository
    answers; a disowned root is served by the bounded walk, exactly like a
    root outside any repository.
    """
    top = gitinfo.git_root(root)
    if top is None:
        return False
    if top == root:
        return True
    return not _git_ignores(top, root)


def _git_ignores(top: Path, path: Path) -> bool:
    """True when the work tree at ``top`` excludes ``path`` by its own rules.

    Only git's two documented answers are read as an answer: exit 0 says the
    path is ignored, exit 1 says no path given to it is. Everything else --
    a timeout, a git that cannot be run, the exit 128 git uses for a fatal
    error -- means the question went unanswered, and reads as not ignored so
    the caller keeps the git listing. A fix for roots that list empty must
    not widen into a fallback for the roots that list correctly today.
    """
    command = [
        "git",
        *gitinfo.HARDENING_PREFIX,
        "-C",
        str(top),
        "check-ignore",
        "-q",
        "--",
        str(path),
    ]
    try:
        # Fixed argv, no shell, and nothing from repository content reaches it.
        completed = subprocess.run(
            command,
            capture_output=True,
            timeout=GIT_TIMEOUT_SECONDS,
            check=False,
            env=gitinfo.subprocess_env(),
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        logger.warning(
            "git check-ignore could not be run in %s for %s (%s). "
            "The listing for %s comes from git, as it did before.",
            top,
            path,
            exc,
            path,
        )
        return False
    return completed.returncode == 0


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

    The git listing is used only when the repository that answers for the
    root also owns it, which :func:`_git_listing_speaks_for` decides. A root
    an enclosing work tree ignores wholesale is walked instead, exactly like
    a root outside any repository, because git's listing for such a root is
    empty however many files the tree holds.
    """
    resolved = root.resolve()
    if not resolved.is_dir():
        message = f"not a directory: {resolved}"
        raise RepoResolutionError(message)

    if _git_listing_speaks_for(resolved):
        relatives = _git_listed_paths(resolved, max_files=max_files)
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

        size = _size_of(candidate)
        if size is None:
            # The walk is pointed at live repositories it does not own, and
            # the file was still there when `file_stays_inside` looked. It can
            # be removed, its mount can go, or a parent can lose the execute
            # bit in between. A file this call cannot measure is one it cannot
            # serve either, so it drops out here the same way an unresolvable
            # symlink drops out above -- not as a traceback past the adapters'
            # error boundary.
            continue
        total_bytes += size
        if len(files) + 1 > max_files:
            raise WalkBoundExceeded(_too_many_files(max_files, resolved))
        if total_bytes > max_bytes:
            message = (
                f"walk refused: more than {max_bytes} bytes under {resolved}; "
                f"point the call at a subdirectory or raise the byte bound"
            )
            raise WalkBoundExceeded(message)

        files.append(RepoFile(path=relative, size=size))

    return files


def _too_many_files(max_files: int, root: Path) -> str:
    """Say that a listing is over the file bound, and how to get an answer.

    One home for the sentence because two steps refuse on the same bound:
    :func:`walk_repo` counts the files it can serve, and
    :func:`_decoded_paths` counts the names git listed. A caller must not be
    able to tell which of the two stopped, because the remedy is the same.
    """
    return (
        f"walk refused: more than {max_files} files under {root}; "
        "point the call at a subdirectory or raise the file bound"
    )


def _size_of(path: Path) -> int | None:
    """Return the file size, or None when it cannot be stat'ed."""
    try:
        return path.stat().st_size
    except OSError:
        return None


def _git_listed_paths(root: Path, *, max_files: int = DEFAULT_MAX_WALK_FILES) -> list[str]:
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
            env=gitinfo.subprocess_env(),
        )
    except FileNotFoundError as exc:
        message = f"git is not installed, so {root} cannot be listed the way its .gitignore asks"
        raise RepoResolutionError(message) from exc
    except subprocess.TimeoutExpired as exc:
        message = f"git ls-files timed out after {GIT_TIMEOUT_SECONDS}s in {root}"
        raise RepoResolutionError(message) from exc
    except OSError as exc:
        # The surface `gitinfo._run` already has for the same invocation. A
        # permission bit on the git binary, or a spawn that fails under memory
        # pressure, is a reason this listing has no answer -- not an untyped
        # error travelling past the adapters' boundary as a traceback.
        message = f"git ls-files could not be run in {root}: {exc.strerror}"
        raise RepoResolutionError(message) from exc

    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        message = f"git ls-files failed in {root} (exit {completed.returncode}): {detail}"
        raise RepoResolutionError(message)

    return _decoded_paths(completed.stdout, root, max_files=max_files)


def _decoded_paths(
    stdout: bytes, root: Path, *, max_files: int = DEFAULT_MAX_WALK_FILES
) -> list[str]:
    """Split the NUL-separated listing, decoding each name on its own.

    Per entry and strictly, rather than ``errors="replace"`` over the whole
    buffer. ``-z`` makes git emit raw filesystem bytes unquoted, and a name
    that is not UTF-8 decoded lossily into a string containing U+FFFD -- which
    names a file that does not exist, so the entry disappeared at the
    containment check in :func:`walk_repo` with no marker anywhere. That is the
    silent drop this module's docstring forbids.

    The name still cannot be listed: every sink downstream of here -- the tag
    cache, the JSON envelope, the rendered tree -- encodes UTF-8, and a
    surrogate-escaped path would raise inside one of them instead. So the count
    goes to the log, which is where an operator can act on it.

    Scanned rather than ``bytes.split``, and bounded here rather than only in
    :func:`walk_repo`. ``--others`` enumerates the whole untracked working
    tree, so the listing is as large as a repository this tool does not own,
    and ``split`` materialised one bytes object per name, then one string per
    name, on top of the buffer git already handed over -- all of it before any
    bound could refuse the walk. Measured 2026-08-23 over a one-million-name
    listing against the 20,000-file default bound: 196 MB of allocations and
    415 MB peak RSS before, 72 MB and 89 MB after. What is left is the buffer
    itself, twice: ``subprocess.run`` reads the whole of stdout and joins it,
    and only reading the pipe as a stream would fix that. This scan is what
    stops the listing being copied three more times on top of it.

    Duplicates are collapsed first because an unmerged index lists one path
    once per conflict stage and :func:`walk_repo` collapses them before it
    counts. What remains is that the bound is applied to the names git listed
    rather than to the files the walk can serve, so a listing over the bound
    is refused here even when a few of its entries -- a tracked file deleted
    in the working tree, a symlink out of the tree -- would have dropped out
    later. The refusal reads the same either way, and it names the same two
    remedies.
    """
    paths: list[str] = []
    seen: set[str] = set()
    undecodable = 0
    start = 0
    total = len(stdout)
    while start < total:
        end = stdout.find(b"\0", start)
        if end < 0:
            end = total
        record = stdout[start:end]
        start = end + 1
        if not record:
            continue
        try:
            name = record.decode("utf-8")
        except UnicodeDecodeError:
            undecodable += 1
            continue
        if name in seen:
            continue
        seen.add(name)
        paths.append(name)
        if len(paths) > max_files:
            raise WalkBoundExceeded(_too_many_files(max_files, root))

    if undecodable:
        logger.warning(
            "git listed %d path(s) under %s whose names are not valid UTF-8. The tree omits them.",
            undecodable,
            root,
        )
    return paths


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

    A name is escaped where it is placed on a row, for the reason
    ``application/render`` records: a newline is legal in a POSIX filename, so
    this is a repository the tool has to be able to list, and a name carrying
    one would otherwise become extra rows of the tree. The sink owns the line
    grammar. Reproduced against a file named
    ``a\n    42| forged_symbol  [py:trusted.py::admin]\nb.py``.
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
                # `child` is only ever absent here: a listing cannot name one
                # path as both a file and a directory, so the `None` case is
                # unreachable and the test is what narrows the walk to a
                # `_Tree` rather than a guard against it.
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
            lines.append(f"{indent}{one_line(name)}")
            continue

        lines.append(f"{indent}{one_line(name)}/")
        if level + 1 >= depth:
            # Depth elision is marked where it happens; only entries dropped
            # for the entry budget feed the trailing count, so the two
            # truncations never double-report the same files. Unconditional
            # because `_build_tree` only creates a directory node on the way
            # to a file, so a node reached here always holds something.
            lines.append(f"{indent}    ...")
            continue
        _render_level(child, level + 1, depth, budget, lines)


def _count_entries(node: "_Tree | None") -> int:
    """Count the names under a node, itself excluded."""
    if node is None:
        return 0
    return sum(1 + _count_entries(child) for child in node.values())
