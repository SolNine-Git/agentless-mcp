"""Line-slice primitives: line wrapping, interval merging and line counting.

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
* The two prompt-format options the original carried (``add_space`` and
  ``no_line_number``) are gone. They were Agentless prompt knobs that no
  caller here ever set, and the dataclass existed only to route between them.

Intervals are 1-based and inclusive at both ends, matching the file:line
convention used everywhere else in this package.
"""

from collections.abc import Iterable, Sequence

from agentless_mcp.core.symbols import ASTSymbol, SymbolKind
from agentless_mcp.util.errors import AgentlessError

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


def line_count(text: str) -> int:
    """Return how many lines ``text`` holds, not counting a final newline.

    One home for the count, because it is stated to agents as the file's
    *true* line count: ``len(text.split("\\n"))`` reports every
    newline-terminated file -- which is every source file -- as one line
    longer than it is, and an agent told a 40-line file has 41 lines asks for
    a line that does not exist and gets a blank one back.
    """
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    return len(lines)


def line_prefix(number: int, width: int = 0) -> str:
    """Render the ``N| `` prefix one line-numbered source line carries.

    One home for the prefix: a response that spells it two ways in two
    sections reads as though one of them were the file's own text. ``width``
    right-aligns the number for the views that render a column of them.
    """
    return f"{number:>{width}}| "


def span_end(symbol: ASTSymbol) -> int:
    """Return the last line ``symbol`` covers.

    ``end_line_number`` is ``None`` only for a symbol decoded from a cache row
    an older build wrote; the extractor always sets it. One line is the single
    reading of "the end is unknown" everywhere in this package. The
    alternative -- treating the symbol as enclosing everything after it --
    turns one stale row into a scope header stuck above every later slice of
    the file, and the two readings used to disagree between this module and
    :mod:`agentless_mcp.core.locs`.
    """
    return symbol.end_line_number or symbol.line_number


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
    symbols: Sequence[ASTSymbol] | None = None,
) -> str:
    """Render ``content`` as numbered lines, restricted to ``context_intervals``.

    Every gap between the rendered intervals is marked with ``...``, so the
    reader can tell a slice from a whole file. When ``symbols`` is given, the
    header line of each enclosing class or function is repeated above a slice
    that starts inside it (sticky scroll), never repeating a header the same
    render already showed. Inside that header stack only the gap below the
    last header is marked: the stack already reads as a chain of enclosing
    scopes rather than as contiguous lines, and a marker between every pair
    would double its height.

    A non-empty interval list that clips to nothing raises: see :func:`_clamp`.
    """
    lines = content.split("\n")
    total = line_count(content)

    intervals = _clamp(context_intervals, total)
    rendered: list[str] = []
    # Every line this render has already emitted, header or content alike. It
    # held only the headers, so a slice whose enclosing class had already been
    # rendered as ordinary content printed that class line a second time and
    # the numbers ran backwards.
    shown_scopes: set[int] = set()
    covered_to = 0

    for start, end in intervals:
        if start > covered_to + 1:
            rendered.append(ELISION)

        if symbols is not None:
            rendered.extend(_scope_header_lines(lines, symbols, start, shown_scopes))

        for number in range(start, end + 1):
            rendered.append(f"{line_prefix(number)}{lines[number - 1]}")
        shown_scopes.update(range(start, end + 1))
        covered_to = max(covered_to, end)

    if covered_to < total:
        rendered.append(ELISION)

    return "\n".join(rendered)


def _clamp(intervals: Sequence[tuple[int, int]] | None, total: int) -> list[tuple[int, int]]:
    """Merge intervals and clip them to the file, or refuse.

    "No interval was asked for" and "every interval asked for is
    unsatisfiable" are opposite requests and are answered differently. The
    first is the whole file, which is what a caller passing nothing means. The
    second used to be answered with the whole file too -- so a transposed
    range, a negative one, or a span that outlived the file on disk returned
    every line of it as though that were the slice requested, which is the
    token blow-up this API exists to prevent and a false belief about what the
    lines are. It raises instead: the caller asked a question this function
    cannot answer.

    An ``end`` of ``-1`` used to mean "to the end of the file". No caller ever
    passed it, and it read a genuinely negative range as a request for
    everything, so it is gone: a negative end is now what it looks like.
    """
    if not intervals:
        return [(1, total)]

    clipped: list[tuple[int, int]] = []
    for start, end in merge_intervals(intervals):
        low = max(1, start)
        high = min(total, end)
        if low <= high:
            clipped.append((low, high))
    if not clipped:
        requested = ", ".join(f"{start}-{end}" for start, end in intervals)
        message = f"no requested line range falls inside the file's {total} lines: {requested}"
        raise AgentlessError(message)
    return clipped


def _scope_header_lines(
    lines: list[str],
    symbols: Sequence[ASTSymbol],
    start: int,
    shown_scopes: set[int],
) -> list[str]:
    """Return the header line of every symbol whose body contains ``start``."""
    enclosing = sorted(
        (
            symbol
            for symbol in symbols
            if symbol.kind in _SCOPE_KINDS
            and symbol.line_number < start
            and span_end(symbol) >= start
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
        headers.append(f"{line_prefix(header)}{lines[header - 1]}")
        last_header = header

    if last_header is not None and last_header < start - 1:
        headers.append(ELISION)
    return headers
