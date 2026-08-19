"""Symbol lookup, body expansion and fan-in.

Three calls, and the split between them is the two-call escalation shape the
BLAgent result argues for: locate cheaply on signatures, then expand only the
handful of symbols the evidence points at. ``find_symbol`` and the map are
the cheap half; ``expand_symbols`` is the escalation, capped at ten bodies so
it stays an escalation rather than a second way to read the repository.

``find_referencing_symbols`` is the asymmetric half of navigation. Callees
come free from reading a body; callers do not, and they are what an error-path
review or a blast-radius question actually needs. Each reference is attributed
to the symbol whose span contains it, so the answer reads as "these functions
call it" rather than as a list of line numbers.

``shared_callers`` is the adjacency pass behind the DRY question: symbols that
the *same* callers also call. A helper that four of your caller's callers
already use is the "we already have a utility for this" signal.
"""

from collections.abc import Iterable
from dataclasses import dataclass, field
from typing import Any

from agentless_mcp.application import render
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core import graph, refs
from agentless_mcp.core.cache import effective_source
from agentless_mcp.core.extractor import Ref, TreeSitterExtractor
from agentless_mcp.core.symbols import (
    ASTSymbol,
    SymbolKind,
    parse_stable_id,
    qualname,
    symbol_stable_id,
)
from agentless_mcp.util.errors import SecurityRefusal
from agentless_mcp.util.fslimits import contained_path, read_bounded

DEFAULT_FIND_LIMIT = 20
DEFAULT_REFS_LIMIT = 50
DEFAULT_EXPAND_LIMIT = 10


@dataclass(frozen=True)
class FindResult:
    """Symbol matches for one query, already capped at the limit."""

    query: str
    cards: tuple[render.SymbolCard, ...]
    total: int
    limit: int

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this result."""
        return {
            "query": self.query,
            "total": self.total,
            "limit": self.limit,
            "matches": [card.as_dict() for card in self.cards],
        }


@dataclass(frozen=True)
class ExpandResult:
    """Full bodies for the requested stable ids, plus the ids that missed."""

    cards: tuple[render.SymbolCard, ...]
    unresolved: tuple[tuple[str, str], ...]

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this expansion."""
        return {
            "symbols": [card.as_dict() for card in self.cards],
            "unresolved": [
                {"stable_id": entry, "reason": reason} for entry, reason in self.unresolved
            ],
        }


@dataclass(frozen=True)
class RefsResult:
    """Fan-in for one target: grouped references and optional adjacency."""

    target: str
    groups: tuple[render.RefGroup, ...]
    total: int
    limit: int
    shared: tuple[render.SharedCaller, ...] = ()

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this result."""
        return {
            "target": self.target,
            "total": self.total,
            "limit": self.limit,
            "groups": [group.as_dict() for group in self.groups],
            "shared_callers": [row.as_dict() for row in self.shared],
        }


class SymbolService:
    """Finds, expands and traces symbols. Holds no per-repository state."""

    def __init__(self, extractor: TreeSitterExtractor) -> None:
        self._extractor = extractor

    def find_symbol(
        self,
        ctx: RepoContext,
        query: str,
        *,
        kind: str | None = None,
        limit: int = DEFAULT_FIND_LIMIT,
    ) -> FindResult:
        """Match a substring or a qualified name across the repository."""
        scan = refs.scan_repo(ctx.root, self._extractor, source=ctx.symbols)
        needle = query.lower()

        matches = [
            (facts, symbol)
            for facts in scan.files
            for symbol in facts.symbols
            if _matches(symbol, needle, kind)
        ]
        matches.sort(key=lambda pair: (*_match_rank(pair[1], needle), pair[0].path))

        cards = tuple(_card(symbol) for _, symbol in matches[:limit])
        return FindResult(query=query, cards=cards, total=len(matches), limit=limit)

    def expand_symbols(
        self,
        ctx: RepoContext,
        stable_ids: list[str],
        *,
        limit: int = DEFAULT_EXPAND_LIMIT,
    ) -> ExpandResult:
        """Return line-numbered full bodies for the named stable ids."""
        cards: list[render.SymbolCard] = []
        unresolved: list[tuple[str, str]] = []

        for raw in stable_ids[:limit]:
            card, reason = self._expand_one(ctx, raw)
            if card is None:
                unresolved.append((raw, reason))
            else:
                cards.append(card)

        for raw in stable_ids[limit:]:
            unresolved.append((raw, f"not expanded: the per-call limit is {limit} symbols"))

        return ExpandResult(cards=tuple(cards), unresolved=tuple(unresolved))

    def find_referencing_symbols(
        self,
        ctx: RepoContext,
        target: str,
        *,
        limit: int = DEFAULT_REFS_LIMIT,
        shared_callers: bool = False,
    ) -> RefsResult:
        """Return the sites that reference ``target``, grouped by file."""
        scan = refs.scan_repo(ctx.root, self._extractor, source=ctx.symbols)
        index = refs.build_ref_index(scan)
        by_path = scan.by_path()

        definitions = _definitions_for(target, index)
        sites = _dedupe(
            site for definition in definitions for site in refs.references_to(index, definition)
        )

        groups = _group_sites(sites[:limit], by_path)
        shared = (
            _shared_callers(sites, definitions, index, by_path, ctx.config.stoplist)
            if shared_callers
            else ()
        )
        return RefsResult(
            target=target,
            groups=groups,
            total=len(sites),
            limit=limit,
            shared=shared,
        )

    def _expand_one(self, ctx: RepoContext, raw: str) -> tuple[render.SymbolCard | None, str]:
        """Resolve one stable id to a card carrying the symbol's whole body."""
        try:
            parsed = parse_stable_id(raw)
        except ValueError as exc:
            return None, str(exc)

        try:
            # The id came from a caller, so its path is foreign data even
            # though this package generated the id in the first place.
            absolute = contained_path(ctx.root, parsed.path)
        except SecurityRefusal as exc:
            return None, str(exc)

        read = read_bounded(absolute)
        if read.text is None:
            return None, f"{parsed.path}: {read.skipped}"

        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(absolute.suffix)
        if language is None:
            return None, f"{parsed.path}: no grammar for this file type"

        symbols = effective_source(ctx.symbols, self._extractor).symbols_for(
            read.text, language, parsed.path
        )
        match = next((symbol for symbol in symbols if qualname(symbol) == parsed.qualname), None)
        if match is None:
            return None, f"{parsed.path} no longer defines {parsed.qualname}"

        lines = read.text.split("\n")
        start = match.line_number
        end = min(len(lines), match.end_line_number or match.line_number)
        body = "\n".join(f"{number}| {lines[number - 1]}" for number in range(start, end + 1))
        return _card(match, body=body), ""


def _matches(symbol: ASTSymbol, needle: str, kind: str | None) -> bool:
    """True when a symbol satisfies both the name query and the kind filter."""
    if kind is not None and symbol.kind.value != kind:
        return False
    return needle in symbol.name.lower() or needle in qualname(symbol).lower()


def _match_rank(symbol: ASTSymbol, needle: str) -> tuple[int, str]:
    """Rank exact names above qualified matches above substring matches."""
    lowered = symbol.name.lower()
    if lowered == needle:
        return (0, lowered)
    if qualname(symbol).lower() == needle:
        return (1, lowered)
    if lowered.startswith(needle):
        return (2, lowered)
    return (3, lowered)


def _card(symbol: ASTSymbol, body: str = "") -> render.SymbolCard:
    """Build the incident card for one symbol."""
    return render.SymbolCard(
        stable_id=symbol_stable_id(symbol),
        path=symbol.module_path,
        start_line=symbol.line_number,
        end_line=symbol.end_line_number or symbol.line_number,
        kind=symbol.kind.value,
        language=symbol.language,
        signature=symbol.signature,
        parent_class=symbol.parent_class,
        body=body,
    )


def _definitions_for(target: str, index: refs.RefIndex) -> list[refs.Definition]:
    """Resolve a refs target -- a stable id or a bare name -- to definitions."""
    try:
        parsed = parse_stable_id(target)
    except ValueError:
        name = target.rpartition(".")[2] or target
        return list(index.definitions.get(name, ()))

    name = parsed.qualname.rpartition(".")[2] or parsed.qualname
    scoped = [
        definition
        for definition in index.definitions.get(name, ())
        if definition.path == parsed.path and qualname(definition.symbol) == parsed.qualname
    ]
    return scoped or list(index.definitions.get(name, ()))


def _dedupe(sites: Iterable[Ref]) -> list[Ref]:
    """Collapse duplicate sites and order them by file then line."""
    unique = {(site.path, site.line, site.name): site for site in sites}
    return [unique[key] for key in sorted(unique)]


def _group_sites(
    sites: list[Ref],
    by_path: dict[str, refs.FileFacts],
) -> tuple[render.RefGroup, ...]:
    """Group reference sites by file and attribute each to its enclosing symbol."""
    grouped: dict[str, list[render.RefSite]] = {}
    for site in sites:
        facts = by_path.get(site.path)
        symbol = refs.enclosing_symbol(facts, site.line) if facts else None
        grouped.setdefault(site.path, []).append(
            render.RefSite(
                line=site.line,
                enclosing=qualname(symbol) if symbol else render.MODULE_LEVEL,
                stable_id=symbol_stable_id(symbol) if symbol else None,
            )
        )
    return tuple(render.RefGroup(path=path, sites=tuple(grouped[path])) for path in sorted(grouped))


@dataclass
class _Adjacency:
    """One candidate symbol and the target's callers that also reference it."""

    definition: refs.Definition
    callers: dict[str, ASTSymbol] = field(default_factory=dict[str, ASTSymbol])

    def add(self, caller: ASTSymbol) -> None:
        """Record one shared caller, keyed so it counts once."""
        self.callers[symbol_stable_id(caller)] = caller

    def row(
        self, stable: str, index: refs.RefIndex, stoplist: frozenset[str]
    ) -> render.SharedCaller:
        """Render this candidate as a ranked adjacency row."""
        symbol = self.definition.symbol
        files = {caller.module_path for caller in self.callers.values()}
        spread = index.files_referencing.get(symbol.name, 1)
        score = (
            len(files)
            * graph.name_multiplier(symbol.name, stoplist)
            / graph.common_name_damping(spread)
        )
        callers = sorted(
            (
                render.CallerRef(
                    qualname=qualname(caller),
                    path=caller.module_path,
                    line=caller.line_number,
                )
                for caller in self.callers.values()
            ),
            key=lambda caller: (caller.path, caller.line),
        )
        return render.SharedCaller(
            stable_id=stable,
            path=self.definition.path,
            line=symbol.line_number,
            overlap=len(self.callers),
            shared_files=len(files),
            score=score,
            callers=tuple(callers),
        )


def _shared_callers(
    sites: list[Ref],
    definitions: list[refs.Definition],
    index: refs.RefIndex,
    by_path: dict[str, refs.FileFacts],
    stoplist: frozenset[str],
) -> tuple[render.SharedCaller, ...]:
    """Rank symbols by how many of the target's callers also reference them.

    The caller set is computed once from the target's own reference sites;
    every other symbol is then scored by how much of that set it shares. A
    symbol four of your five callers already use is the DRY signal worth
    surfacing.

    Two corrections make that ranking usable on a real repository, and both
    are the treatment :mod:`agentless_mcp.core.graph` already applies to the
    same problem:

    * **Files, not sites.** Four callers in one module are one team's habit;
      four callers across four modules are a utility. The score counts
      distinct files, so a cluster of callers inside one file cannot outvote
      genuine spread.
    * **Common names are damped.** Fan-in is name-matched, so a candidate
      named ``get`` or ``id`` shares callers with everything. Dividing by
      :func:`~agentless_mcp.core.graph.common_name_damping` of the name's
      repository-wide spread, and applying the stoplist multiplier, is what
      keeps an incidentally shared common name below a genuinely shared
      helper it out-counts.
    """
    target_ids = {symbol_stable_id(definition.symbol) for definition in definitions}

    callers: dict[str, ASTSymbol] = {}
    for site in sites:
        facts = by_path.get(site.path)
        symbol = refs.enclosing_symbol(facts, site.line) if facts else None
        if symbol is not None:
            callers[symbol_stable_id(symbol)] = symbol

    shared: dict[str, _Adjacency] = {}
    for caller_id, caller in sorted(callers.items()):
        facts = by_path.get(caller.module_path)
        if facts is None:
            continue
        end = caller.end_line_number or caller.line_number
        names = {ref.name for ref in facts.refs if caller.line_number <= ref.line <= end}
        for name in sorted(names):
            for definition in index.definitions.get(name, ()):
                candidate = symbol_stable_id(definition.symbol)
                if candidate in target_ids or candidate == caller_id:
                    continue
                shared.setdefault(candidate, _Adjacency(definition)).add(caller)

    rows = [
        entry.row(candidate, index, stoplist)
        for candidate, entry in shared.items()
        if len(entry.callers) > 1
    ]
    rows.sort(key=lambda row: (-row.score, -row.shared_files, row.stable_id))
    return tuple(rows)


def kind_names() -> tuple[str, ...]:
    """Return the symbol kinds a caller may filter on."""
    return tuple(kind.value for kind in SymbolKind)
