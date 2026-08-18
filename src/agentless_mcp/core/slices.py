"""Line-slice primitives: line wrapping and interval merging.

Ported from Agentless (``agentless/util/preprocess_data.py``) with three
changes, each of which the tests pin:

* The trailing ``...`` marker is computed from the whole set of intervals
  instead of whatever the loop variable happened to hold on the last
  iteration. In the original, an earlier interval reaching the end of the
  file still produced a trailing elision marker.
* Intervals are merged and sorted here rather than assumed pre-merged, and
  the caller's list is never mutated in place.
* Sticky-scroll scope headers come from extracted symbols instead of the
  ``startswith("class ")`` string heuristic, which only ever worked on
  Python and mislabelled any line that happened to start that way.

Intervals are 1-based and inclusive at both ends, matching the file:line
convention used everywhere else in this package.
"""

from collections.abc import Iterable, Sequence
from dataclasses import dataclass

from agentless_mcp.core.symbols import ASTSymbol, SymbolKind

ELISION = "..."

# The kinds whose header line is worth repeating above a slice: they are the
# scopes a reader needs to know they are inside of.
_SCOPE_KINDS = frozenset(
    {
        SymbolKind.CLASS,
        SymbolKind.FUNCTION,
        SymbolKind.METHOD,
        SymbolKind.PROTOCOL,
        SymbolKind.DATACLASS,
        SymbolKind.ENUM,
    }
)


@dataclass(frozen=True)
class _LineFormat:
    """How a single source line is rendered, matching the Agentless prompts."""

    add_space: bool
    no_line_number: bool

    def render(self, number: int, line: str) -> str:
        """Format one source line."""
        if self.no_line_number:
            return line
        if self.add_space:
            return f"{number}| {line} "
        return f"{number}|{line}"


def merge_intervals(intervals: Iterable[tuple[int, int]]) -> list[tuple[int, int]]:
    """Merge overlapping 1-based inclusive intervals, sorted by start.

    Takes any iterable and returns a new list: the caller's sequence is left
    untouched, unlike the original which sorted it in place.
    """
    ordered = sorted(intervals, key=lambda interval: interval[0])
    if not ordered:
        return []

    merged: list[tuple[int, int]] = [ordered[0]]
    for current in ordered[1:]:
        last = merged[-1]
        if current[0] <= last[1]:
            merged[-1] = (last[0], max(last[1], current[1]))
        else:
            merged.append(current)
    return merged


def line_wrap_content(
    content: str,
    context_intervals: Sequence[tuple[int, int]] | None = None,
    *,
    add_space: bool = False,
    no_line_number: bool = False,
    symbols: Sequence[ASTSymbol] | None = None,
) -> str:
    """Render ``content`` as numbered lines, restricted to ``context_intervals``.

    Every stretch of content that is not rendered is marked with ``...``, so
    the reader can tell a slice from a whole file. When ``symbols`` is given,
    the header line of each enclosing class or function is repeated above a
    slice that starts inside it (sticky scroll), never repeating a header the
    same render already showed.
    """
    lines = content.split("\n")
    total = len(lines)

    line_format = _LineFormat(add_space=add_space, no_line_number=no_line_number)
    intervals = _clamp(context_intervals, total)
    rendered: list[str] = []
    shown_scopes: set[int] = set()
    covered_to = 0

    for start, end in intervals:
        if start > covered_to + 1:
            rendered.append(ELISION)

        if symbols is not None:
            rendered.extend(_scope_header_lines(lines, symbols, start, shown_scopes, line_format))

        for number in range(start, end + 1):
            rendered.append(line_format.render(number, lines[number - 1]))
        covered_to = max(covered_to, end)

    if covered_to < total:
        rendered.append(ELISION)

    return "\n".join(rendered)


def _clamp(intervals: Sequence[tuple[int, int]] | None, total: int) -> list[tuple[int, int]]:
    """Merge intervals and clip them to the file, defaulting to the whole file."""
    if not intervals:
        return [(1, total)]

    clipped: list[tuple[int, int]] = []
    for start, end in merge_intervals(intervals):
        low = max(1, start)
        high = total if end == -1 else min(total, end)
        if low <= high:
            clipped.append((low, high))
    return clipped or [(1, total)]


def _scope_header_lines(
    lines: list[str],
    symbols: Sequence[ASTSymbol],
    start: int,
    shown_scopes: set[int],
    line_format: _LineFormat,
) -> list[str]:
    """Return the header line of every symbol whose body contains ``start``."""
    enclosing = sorted(
        (
            symbol
            for symbol in symbols
            if symbol.kind in _SCOPE_KINDS
            and symbol.line_number < start
            and (symbol.end_line_number is None or symbol.end_line_number >= start)
        ),
        key=lambda symbol: symbol.line_number,
    )

    headers: list[str] = []
    last_header: int | None = None
    for symbol in enclosing:
        header = symbol.line_number
        if header in shown_scopes or header > len(lines):
            continue
        shown_scopes.add(header)
        headers.append(line_format.render(header, lines[header - 1]))
        last_header = header

    if last_header is not None and last_header < start - 1:
        headers.append(ELISION)
    return headers
