"""Resolve Agentless-style location strings against extracted symbols.

Ported from ``agentless/util/preprocess_data.py::transfer_arb_locs_to_locs``
(L113-322) and rewired from that function's Python-only `parse_python_file`
structure onto :class:`~agentless_mcp.core.symbols.ASTSymbol` qualified names,
which makes the same grammar of locations work for every language the
extractor supports.

The accepted forms, and the implicit cases the original had that the tests
transcribe:

* ``class: Invoice`` -- resolves the class, and *becomes the current class*
  for the locations that follow it. That state is the reason a bare
  ``function: total`` after a ``class:`` line resolves to a method.
* ``class: Invoice.total`` -- the original's first branch requires the name to
  have no dot, so a dotted ``class:`` falls through to the function branch and
  is treated as a method. Kept, because model output uses both spellings.
* ``function: total`` -- a module-level function; failing that the current
  class's method; failing that a method of *any* class, accepted only when
  exactly one class defines it.
* ``function: Invoice.total`` and the bare ``Invoice.total`` -- a method,
  looked up by qualified name.
* ``line: 42`` -- a single line. Trailing text after the number is ignored,
  as in the original.
* ``variable: MAX_ITEMS`` -- a module-level constant, which the extractor
  already produces as a CONSTANT symbol.

Four deliberate departures from the original:

* Nothing is dropped silently. The original appended to ``unrecognized_locs``
  in some branches, ``continue``d in others and fell through without a trace
  in the ambiguous-method case. Here every location that does not resolve
  comes back in :attr:`LocResolution.unrecognized` with a reason.
* Intervals are 1-based and clamped to ``[1, total_lines]``. The original
  clamped the low end to 0, which is not a line.
* Matched symbols come back as stable ids as well as spans, so the caller can
  hand them straight to ``expand_symbols``.
* ``line:`` and ``variable:`` are dispatched before the dotted-name
  heuristic, so ``line: 7 in Invoice.total`` is a line rather than a failed
  method lookup. Nothing else moves: see :func:`_resolve_one`.
"""

from collections.abc import Sequence
from dataclasses import dataclass

from agentless_mcp.core.slices import merge_intervals, span_end
from agentless_mcp.core.symbols import (
    ASTSymbol,
    SymbolKind,
    id_qualname,
    split_ordinal,
    stable_id,
)

DEFAULT_CONTEXT_LINES = 10

# The kinds a `class:` location can name. Interfaces, enums, protocols and
# dataclasses are all "the thing methods hang off" as far as a location goes.
CLASS_KINDS = frozenset(
    {
        SymbolKind.CLASS,
        SymbolKind.DATACLASS,
        SymbolKind.ENUM,
        SymbolKind.PROTOCOL,
    }
)

_CLASS_PREFIX = "class:"
_FUNCTION_PREFIXES = ("function:", "method:")
_LINE_PREFIX = "line:"
_VARIABLE_PREFIX = "variable:"


@dataclass(frozen=True)
class LocTarget:
    """The one file a set of locations is resolved against."""

    path: str
    language: str
    symbols: tuple[ASTSymbol, ...]
    total_lines: int


@dataclass(frozen=True)
class UnrecognizedLoc:
    """A location string that resolved to nothing, and why."""

    loc: str
    reason: str


@dataclass(frozen=True)
class LocResolution:
    """What a set of location strings resolved to.

    ``spans`` are the symbols' own line ranges; ``intervals`` are those spans
    widened by the context window and merged, which is what a slice renders.
    """

    stable_ids: tuple[str, ...]
    spans: tuple[tuple[int, int], ...]
    intervals: tuple[tuple[int, int], ...]
    unrecognized: tuple[UnrecognizedLoc, ...]


@dataclass(frozen=True)
class _Hit:
    """One location's outcome: a span, an id, a class to remember, or a reason."""

    span: tuple[int, int] | None = None
    stable: str | None = None
    current_class: str | None = None
    reason: str | None = None


def resolve_locs(
    locs: Sequence[str] | str,
    target: LocTarget,
    *,
    context: int = DEFAULT_CONTEXT_LINES,
) -> LocResolution:
    """Resolve location strings against one file's symbols."""
    spans: list[tuple[int, int]] = []
    ids: list[str] = []
    unrecognized: list[UnrecognizedLoc] = []
    current_class = ""

    for loc in _split(locs):
        hit = _resolve_one(loc, target, current_class)
        if hit.current_class is not None:
            current_class = hit.current_class
        if hit.reason is not None:
            unrecognized.append(UnrecognizedLoc(loc=loc, reason=hit.reason))
            continue
        if hit.span is not None:
            spans.append(hit.span)
        if hit.stable is not None:
            ids.append(hit.stable)

    intervals = merge_intervals(_widen(spans, target, context))

    return LocResolution(
        stable_ids=tuple(dict.fromkeys(ids)),
        spans=tuple(spans),
        intervals=tuple(intervals),
        unrecognized=tuple(unrecognized),
    )


def _widen(
    spans: Sequence[tuple[int, int]],
    target: LocTarget,
    context: int,
) -> list[tuple[int, int]]:
    """Widen each span by the context window, clamped to the file at both ends.

    A span whose whole widened range lies past the last line yields no
    interval at all. That happens when a symbol's line numbers outlived the
    file on disk -- a cache row written before an edit -- and clamping only
    the high end used to turn it into a reversed interval, which the renderer
    then answered with the entire file. The span itself is still reported, so
    the location is not dropped silently; there is simply nothing to render
    for it.
    """
    widened: list[tuple[int, int]] = []
    for start, end in spans:
        low = max(1, start - context)
        high = min(target.total_lines, end + context) if target.total_lines else end + context
        if low <= high:
            widened.append((low, high))
    return widened


def _split(locs: Sequence[str] | str) -> list[str]:
    """Flatten the accepted input shapes into one location per entry.

    A single string, a list of strings and a list of multi-line blocks are all
    valid input: model output arrives as a block, a CLI passes one flag per
    location, and a tool call passes a list.
    """
    blocks = [locs] if isinstance(locs, str) else list(locs)
    return [line.strip() for block in blocks for line in block.splitlines() if line.strip()]


def _resolve_one(loc: str, target: LocTarget, current_class: str) -> _Hit:
    """Dispatch one location string to the branch that understands it."""
    if loc.startswith(_CLASS_PREFIX) and "." not in loc:
        return _resolve_class(loc.partition(":")[2].strip(), target)
    # `line:` and `variable:` are tested before the dotted-name heuristic. In
    # the original the heuristic came first, so `line: 7 in Invoice.total`
    # -- a line number with an explanatory tail -- was read as a method
    # lookup. Reordering cannot affect `class: A.b`, which is a dotted string
    # with neither prefix and still reaches the function branch below.
    if loc.startswith(_LINE_PREFIX):
        return _resolve_line(loc.partition(":")[2].strip(), target)
    if loc.startswith(_VARIABLE_PREFIX):
        return _resolve_variable(loc.partition(":")[2].strip(), target)
    if loc.startswith(_FUNCTION_PREFIXES) or "." in loc:
        return _resolve_function(loc.partition(":")[2].strip() or loc, target, current_class)
    return _Hit(reason="unknown location form: expected class:, function:, line: or variable:")


def _resolve_class(name: str, target: LocTarget) -> _Hit:
    """Resolve ``class: X`` and remember X for the locations that follow."""
    matches = [
        symbol for symbol in target.symbols if symbol.kind in CLASS_KINDS and symbol.name == name
    ]
    if not matches:
        return _Hit(reason=f"no class named {name!r} in {target.path}")
    return _Hit(span=_span(matches[0]), stable=_id(matches[0], target), current_class=name)


def _resolve_function(name: str, target: LocTarget, current_class: str) -> _Hit:
    """Resolve a function, a qualified method, or a bare method name.

    A trailing ``#2``/``#3`` is the collision ordinal a stable id carries when
    a file spells one qualified name twice (see
    :func:`~agentless_mcp.core.symbols.split_ordinal`). It is split off here,
    at the one door every function-shaped location comes through, so that a
    location taken straight from an id addresses the symbol that id names
    rather than the first symbol sharing its name.
    """
    wanted, ordinal = split_ordinal(name)
    if "." in wanted:
        class_name, _, method_name = wanted.rpartition(".")
        return _resolve_method(class_name, method_name, target, ordinal)

    functions = [
        symbol
        for symbol in target.symbols
        if symbol.kind == SymbolKind.FUNCTION and symbol.name == wanted
    ]
    if functions:
        return _pick(functions, wanted, ordinal, target)

    if current_class:
        # The current class is an implicit scope, not one the caller named:
        # on a miss, fall through to the any-class search rather than surface
        # a reason blaming a class the location never mentioned.
        hit = _resolve_method(current_class, wanted, target, ordinal)
        if hit.reason is None:
            return hit

    return _resolve_unqualified_method(wanted, ordinal, target)


def _resolve_method(class_name: str, method_name: str, target: LocTarget, ordinal: int) -> _Hit:
    """Resolve ``Class.method`` against the class's own methods."""
    if not any(
        symbol.kind in CLASS_KINDS and symbol.name == class_name for symbol in target.symbols
    ):
        return _Hit(reason=f"no class named {class_name!r} in {target.path}")

    methods = [
        symbol
        for symbol in target.symbols
        if symbol.parent_class == class_name and symbol.name == method_name
    ]
    if not methods:
        return _Hit(reason=f"class {class_name!r} has no member named {method_name!r}")
    return _pick(methods, f"{class_name}.{method_name}", ordinal, target)


def _resolve_unqualified_method(name: str, ordinal: int, target: LocTarget) -> _Hit:
    """Resolve a bare method name, but only when exactly one class defines it.

    The original accepted the single-match case and fell through silently when
    several classes defined the name. Several matches is reported here: an
    ambiguous location is a question for the caller, not a coin flip.
    """
    methods = [symbol for symbol in target.symbols if symbol.parent_class and symbol.name == name]
    if not methods:
        return _Hit(reason=f"no function or method named {name!r} in {target.path}")
    if len(methods) > 1:
        owners = ", ".join(sorted({symbol.parent_class for symbol in methods}))
        return _Hit(reason=f"{name!r} is ambiguous: defined in {owners}")
    return _pick(methods, name, ordinal, target)


def _pick(matches: list[ASTSymbol], name: str, ordinal: int, target: LocTarget) -> _Hit:
    """Choose the ordinal-th of several same-named matches, or say it is absent."""
    chosen = next((symbol for symbol in matches if symbol.duplicate_index == ordinal), None)
    if chosen is None:
        return _Hit(
            reason=f"{target.path} defines {len(matches)} symbols named {name!r}, "
            f"so there is no number {ordinal + 1}"
        )
    return _Hit(span=_span(chosen), stable=_id(chosen, target))


def _resolve_line(text: str, target: LocTarget) -> _Hit:
    """Resolve ``line: N``, ignoring anything after the number."""
    head = text.split()
    if not head:
        return _Hit(reason="line location carries no number")
    try:
        number = int(head[0])
    except ValueError:
        return _Hit(reason=f"{head[0]!r} is not a line number")

    if number < 1 or (target.total_lines and number > target.total_lines):
        return _Hit(reason=f"line {number} is outside {target.path} (1-{target.total_lines})")
    return _Hit(span=(number, number))


def _resolve_variable(text: str, target: LocTarget) -> _Hit:
    """Resolve ``variable: NAME`` against module-level constants."""
    names = text.split()
    if not names:
        return _Hit(reason="variable location carries no name")

    name = names[0]
    matches = [
        symbol
        for symbol in target.symbols
        if symbol.kind == SymbolKind.CONSTANT and symbol.name == name
    ]
    if not matches:
        return _Hit(reason=f"no module-level constant named {name!r} in {target.path}")
    return _Hit(span=_span(matches[0]), stable=_id(matches[0], target))


def _span(symbol: ASTSymbol) -> tuple[int, int]:
    """Return the symbol's inclusive 1-based line span."""
    return (symbol.line_number, span_end(symbol))


def _id(symbol: ASTSymbol, target: LocTarget) -> str:
    """Build the stable id of a matched symbol under the target's path."""
    return stable_id(target.language, target.path, id_qualname(symbol))
