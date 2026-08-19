"""Zoom levels over one repository: tree, skeleton, slice, location resolution.

The funnel the Agentless pipeline validated is a sequence of zoom levels, and
this is the read side of it: the directory tree says what exists, a skeleton
says what each file declares, a slice says what a specific range actually
does. The discipline is that each step is *narrower* than the last, so an
agent that starts at the tree and ends at a slice has paid for one view at
each level instead of a whole repository at the finest one.

Two defaults carry research behind them. Skeletons drop docstrings and
comments unless asked: it is the cheapest large token saving available and it
removes the prompt-injection surface of an analysed repository in the same
decision. And slices are symbol- or line-addressed with sticky-scroll scope
headers, because a range without its enclosing signature reads as if it were
top level.

Every caller-supplied path crosses ``fslimits.contained_path`` before it is
opened, so a path argument cannot walk out of the repository it was scoped to.
"""

from collections.abc import Sequence
from dataclasses import dataclass
from typing import Any

from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core.cache import effective_source
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.locs import DEFAULT_CONTEXT_LINES, LocResolution, LocTarget, resolve_locs
from agentless_mcp.core.skeleton import skeletonize
from agentless_mcp.core.slices import line_count, line_wrap_content
from agentless_mcp.core.symbols import ASTSymbol
from agentless_mcp.core.treewalk import (
    DEFAULT_MAX_ENTRIES,
    DEFAULT_RENDER_DEPTH,
    render_tree,
    walk_repo,
)
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import AtlasError, RepoResolutionError
from agentless_mcp.util.fslimits import contained_path, read_bounded

# What a caller is told about a line range that is not one. Shaped like
# `MESSAGES.slice_range_beyond_file`, because "you asked for lines 60-30" and
# "you asked for lines past the end" are the same kind of answer: the range is
# named back, with what a satisfiable one looks like.
SLICE_RANGE_NOT_A_RANGE = (
    "unsatisfiable: line range {start}-{end} is not a range in {path}: "
    "a start of 1 or more, and an end at or after the start"
)


@dataclass(frozen=True)
class TreeView:
    """A rendered directory tree plus the counts behind it."""

    text: str
    file_count: int
    depth: int
    max_entries: int

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this tree."""
        return {
            "tree": self.text,
            "file_count": self.file_count,
            "depth": self.depth,
            "max_entries": self.max_entries,
        }


@dataclass(frozen=True)
class FileView:
    """One file rendered at some zoom level, or the reason it was not."""

    path: str
    language: str
    text: str
    error: str = ""

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this file view."""
        record: dict[str, Any] = {"path": self.path, "language": self.language, "text": self.text}
        if self.error:
            record["error"] = self.error
        return record


@dataclass(frozen=True)
class LocationView:
    """A resolved set of location strings, ready to render or to slice."""

    path: str
    resolution: LocResolution
    text: str

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this resolution."""
        return {
            "path": self.path,
            "stable_ids": list(self.resolution.stable_ids),
            "spans": [list(span) for span in self.resolution.spans],
            "intervals": [list(interval) for interval in self.resolution.intervals],
            "unrecognized": [
                {"loc": entry.loc, "reason": entry.reason} for entry in self.resolution.unrecognized
            ],
            "slice": self.text,
        }


def _check_context(context: int) -> None:
    """Refuse a context window that cannot widen anything.

    A negative context narrows the interval it is applied to, and past the
    interval's own width it inverts it. The caller asked for lines around a
    range; there is no answer to "the lines around it, minus fifty".
    """
    if context < 0:
        message = f"context must not be negative, got {context}"
        raise AtlasError(message)


def _satisfiable(start: int, end: int, total: int) -> bool:
    """True when a range names lines this file can answer with, even in part.

    Three conditions and not one: the range starts at line 1 or later, ends at
    or after it starts, and starts inside the file. Keeping only the last of
    them was a guard on a proxy rather than on the invariant -- an inverted or
    negative range passed it, widened into an interval that clipped to
    nothing, and was answered with the whole file.
    """
    return 1 <= start <= min(end, total)


def _unsatisfiable(start: int, end: int, path: str, total: int) -> str:
    """Say why one requested range cannot be answered, naming the range."""
    if start > total:
        return MESSAGES.slice_range_beyond_file.format(start=start, end=end, path=path, total=total)
    return SLICE_RANGE_NOT_A_RANGE.format(start=start, end=end, path=path)


class ViewService:
    """Renders the tree, skeleton, slice and location views of a repository."""

    def __init__(self, extractor: TreeSitterExtractor) -> None:
        self._extractor = extractor

    def tree(
        self,
        ctx: RepoContext,
        *,
        depth: int = DEFAULT_RENDER_DEPTH,
        max_entries: int = DEFAULT_MAX_ENTRIES,
    ) -> TreeView:
        """Render the gitignore-aware directory tree."""
        files = walk_repo(ctx.root)
        return TreeView(
            text=render_tree(files, depth=depth, max_entries=max_entries),
            file_count=len(files),
            depth=depth,
            max_entries=max_entries,
        )

    def skeleton(
        self,
        ctx: RepoContext,
        paths: Sequence[str],
        *,
        docstrings: bool = False,
        numbered: bool = False,
    ) -> list[FileView]:
        """Render each named file as signatures with elided bodies."""
        views: list[FileView] = []
        for raw in paths:
            resolved, language, text, error = self._load(ctx, raw)
            if error:
                views.append(FileView(path=raw, language=language, text="", error=error))
                continue
            try:
                rendered = skeletonize(text, language, docstrings=docstrings, number_lines=numbered)
            except AtlasError as exc:
                views.append(FileView(path=resolved, language=language, text="", error=str(exc)))
                continue
            views.append(FileView(path=resolved, language=language, text=rendered))
        return views

    def read_slice(
        self,
        ctx: RepoContext,
        path: str,
        *,
        intervals: Sequence[tuple[int, int]] = (),
        context: int = DEFAULT_CONTEXT_LINES,
    ) -> FileView:
        """Render numbered lines for the given intervals, with scope headers.

        An interval this file cannot satisfy is reported per item -- the way
        :meth:`resolve_locations` reports an unrecognized loc -- instead of
        being clipped away, which would leave no intervals and render the
        whole file as if it were the requested slice. An interval that merely
        runs past the end keeps its clamped tail.

        "Satisfiable" is the whole invariant and not a proxy for it: a range
        starts at line 1 or later, ends at or after it starts, and starts
        inside the file. Checking only the last of the three let a transposed
        range, a negative one, or a negative ``context`` widen into an
        inverted interval that clipped to nothing -- and the primitive
        underneath answered that with every line of the file, which is exactly
        the blow-up this method exists to prevent. The service owns the check
        because both front doors call it and only one of them validates.
        """
        _check_context(context)
        resolved, language, text, error = self._load(ctx, path)
        if error:
            return FileView(path=path, language=language, text="", error=error)

        total = line_count(text)
        satisfiable = [(start, end) for start, end in intervals if _satisfiable(start, end, total)]
        reports = [
            _unsatisfiable(start, end, resolved, total)
            for start, end in intervals
            if not _satisfiable(start, end, total)
        ]
        if intervals and not satisfiable:
            return FileView(path=resolved, language=language, text="\n".join(reports) + "\n")

        widened = [
            (max(1, start - context), min(total, end + context)) for start, end in satisfiable
        ]
        symbols = self._symbols(ctx, text, language, resolved)
        rendered = line_wrap_content(text, widened if intervals else None, symbols=symbols)
        body = "\n".join([*reports, rendered]) if reports else rendered
        return FileView(path=resolved, language=language, text=body + "\n")

    def resolve_locations(
        self,
        ctx: RepoContext,
        path: str,
        locs: Sequence[str],
        *,
        context: int = DEFAULT_CONTEXT_LINES,
    ) -> LocationView:
        """Resolve ``class:``/``function:``/``line:`` strings against one file."""
        _check_context(context)
        resolved, language, text, error = self._load(ctx, path)
        if error:
            raise RepoResolutionError(error)

        total = line_count(text)
        symbols = self._symbols(ctx, text, language, resolved)
        resolution = resolve_locs(
            list(locs),
            LocTarget(path=resolved, language=language, symbols=tuple(symbols), total_lines=total),
            context=context,
        )
        rendered = (
            line_wrap_content(text, list(resolution.intervals), symbols=symbols) + "\n"
            if resolution.intervals
            else ""
        )
        return LocationView(path=resolved, resolution=resolution, text=rendered)

    def _load(self, ctx: RepoContext, path: str) -> tuple[str, str, str, str]:
        """Return (relative path, language, text, error) for one repository file."""
        absolute = contained_path(ctx.root, path)
        relative = absolute.relative_to(ctx.root).as_posix()
        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(absolute.suffix, "")
        if not language:
            return relative, "", "", f"{relative}: no grammar for this file type"

        read = read_bounded(absolute)
        if read.text is None:
            return relative, language, "", f"{relative}: {read.skipped}"
        return relative, language, read.text, ""

    def _symbols(self, ctx: RepoContext, text: str, language: str, path: str) -> list[ASTSymbol]:
        """Get the symbols a slice uses for its sticky-scroll headers."""
        source = effective_source(ctx.symbols, self._extractor)
        return list(source.symbols_for(text, language, path))
