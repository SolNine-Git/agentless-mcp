"""Path containment, symlink-escape detection and walk/read bounds.

Every path that arrives from outside the process crosses into the package
through :func:`contained_path`, and every traversal of an analysed repository
goes through :func:`bounded_walk`. Both refuse rather than clamp: a caller
that asked for something outside the root, or for a tree larger than the
configured bounds, gets an error naming the bound it hit, never a silently
truncated answer.

Containment is canonicalize-then-check: resolve the candidate (following
symlinks), then test it against the resolved root. Checking the textual form
first and resolving later is the pattern behind most path-traversal CVEs in
this class of tool.
"""

import os
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path

from agentless_mcp.util.errors import RepoResolutionError, SecurityRefusal, WalkBoundExceeded

DEFAULT_MAX_DEPTH = 20
DEFAULT_MAX_FILES = 20_000
DEFAULT_MAX_BYTES = 200_000_000
DEFAULT_MAX_FILE_BYTES = 1_000_000


def contained_path(root: Path, candidate: str) -> Path:
    """Resolve ``candidate`` against ``root`` and refuse anything outside it.

    ``candidate`` may be relative (joined onto the root) or absolute (accepted
    only when it already lives under the root). Traversal segments, symlinks
    pointing out of the tree and, on Windows, a different drive all resolve to
    a path outside the root and are refused.

    The refusal names the resolved form only. The raw argument is never echoed
    back: error text is an output channel like any other.

    Every refusal is a :class:`SecurityRefusal`, including the one for a string
    the filesystem cannot name at all: this is the boundary the adapters catch
    on, so an untyped stdlib error escaping it would escape them too.
    """
    resolved_root = root.resolve()
    joined = Path(candidate) if Path(candidate).is_absolute() else resolved_root / candidate
    try:
        resolved = joined.resolve()
    except (ValueError, OSError) as exc:
        # A NUL byte or an otherwise unnameable path: JSON tool arguments can
        # carry both, and the form is not repeated back.
        message = "path refused: not a usable filesystem path"
        raise SecurityRefusal(message) from exc

    if not _is_within(resolved, resolved_root):
        message = f"path refused: resolved to {resolved}, which is outside the root {resolved_root}"
        raise SecurityRefusal(message)

    if resolved.exists():
        # Re-resolve strictly: an existing final component that is a symlink is
        # only proven safe once the real file it names has been resolved.
        strict = resolved.resolve(strict=True)
        if not _is_within(strict, resolved_root):
            message = (
                f"path refused: resolved to {strict}, which is outside the root {resolved_root}"
            )
            raise SecurityRefusal(message)
        return strict

    return resolved


def _is_within(path: Path, root: Path) -> bool:
    """Return True when ``path`` is ``root`` itself or lives under it."""
    return path == root or root in path.parents


@dataclass(frozen=True)
class BoundedRead:
    """The outcome of a size-capped file read.

    Exactly one of ``text`` and ``skipped`` is set. ``skipped`` carries the
    reason so a caller can report the file it did not read instead of
    presenting a short answer as a complete one.
    """

    path: Path
    text: str | None
    skipped: str | None


def read_bounded(path: Path, max_bytes: int = DEFAULT_MAX_FILE_BYTES) -> BoundedRead:
    """Read ``path`` as UTF-8, skipping it when it exceeds ``max_bytes``.

    Skip-with-report, not raise: one oversized or unreadable file in a
    repository must not fail a whole traversal, but it must also never pass
    silently as an empty file.
    """
    try:
        size = path.stat().st_size
    except OSError as exc:
        return BoundedRead(path=path, text=None, skipped=f"unreadable: {exc.strerror}")

    if size > max_bytes:
        return BoundedRead(
            path=path,
            text=None,
            skipped=f"skipped: {size} bytes exceeds the per-file cap of {max_bytes} bytes",
        )

    try:
        data = path.read_bytes()
    except OSError as exc:
        return BoundedRead(path=path, text=None, skipped=f"unreadable: {exc.strerror}")

    return BoundedRead(path=path, text=data.decode("utf-8", errors="replace"), skipped=None)


def bounded_walk(
    root: Path,
    *,
    max_depth: int = DEFAULT_MAX_DEPTH,
    max_files: int = DEFAULT_MAX_FILES,
    max_bytes: int = DEFAULT_MAX_BYTES,
    include: Callable[[Path], bool] | None = None,
) -> Iterator[Path]:
    """Yield every file under ``root``, refusing to walk past the bounds.

    ``include`` receives each candidate path relative to the root; only files
    it accepts are yielded and counted. Symlinked directories are never
    descended into and symlinked files whose target leaves the root are
    skipped, so the walk cannot be steered outside the tree it was given.
    Directories already visited (by device and inode) are pruned, which stops
    a bind-mount or hardlink cycle from looping forever.

    Gitignore awareness lives in :mod:`agentless_mcp.core.treewalk`, not here:
    this function is the security bound, and a bound that consults repository
    content for its answer is not a bound.
    """
    resolved_root = root.resolve()
    if not resolved_root.is_dir():
        message = f"not a directory: {resolved_root}"
        raise RepoResolutionError(message)

    seen_dirs: set[tuple[int, int]] = set()
    files_yielded = 0
    bytes_yielded = 0

    for dirpath, dirnames, filenames in os.walk(resolved_root, followlinks=False):
        current = Path(dirpath)
        depth = len(current.relative_to(resolved_root).parts)
        if depth > max_depth:
            message = (
                f"walk refused: directory depth {depth} exceeds the limit of {max_depth} "
                f"under {resolved_root}; point the call at a subdirectory instead"
            )
            raise WalkBoundExceeded(message)

        if not _claim_directory(current, seen_dirs):
            dirnames[:] = []
            continue

        dirnames[:] = sorted(name for name in dirnames if not (current / name).is_symlink())

        for name in sorted(filenames):
            candidate = current / name
            if not file_stays_inside(candidate, resolved_root):
                continue

            relative = candidate.relative_to(resolved_root)
            if include is not None and not include(relative):
                continue

            files_yielded += 1
            if files_yielded > max_files:
                message = (
                    f"walk refused: more than {max_files} files under {resolved_root}; "
                    f"point the call at a subdirectory or raise the file bound"
                )
                raise WalkBoundExceeded(message)

            bytes_yielded += _size_of(candidate)
            if bytes_yielded > max_bytes:
                message = (
                    f"walk refused: more than {max_bytes} bytes under {resolved_root}; "
                    f"point the call at a subdirectory or raise the byte bound"
                )
                raise WalkBoundExceeded(message)

            yield candidate


def _claim_directory(directory: Path, seen: set[tuple[int, int]]) -> bool:
    """Record ``directory`` by (device, inode); False when it was seen before."""
    try:
        info = directory.stat()
    except OSError:
        return False
    key = (info.st_dev, info.st_ino)
    if key in seen:
        return False
    seen.add(key)
    return True


def file_stays_inside(candidate: Path, root: Path) -> bool:
    """Return True for regular files whose real path is still under ``root``.

    Public because both traversals need it and one containment rule is the
    point: :mod:`agentless_mcp.core.treewalk` asks the same question of the
    paths git lists that :func:`bounded_walk` asks of the ones it finds.
    """
    if candidate.is_symlink():
        try:
            target = candidate.resolve(strict=True)
        except OSError:
            return False
        return _is_within(target, root)
    return candidate.is_file()


def _size_of(path: Path) -> int:
    """Return the file size, or 0 when it cannot be stat'ed."""
    try:
        return path.stat().st_size
    except OSError:
        return 0
