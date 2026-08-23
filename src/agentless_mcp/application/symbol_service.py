"""Symbol lookup, body expansion and fan-in.

Three calls, and the split between them is the two-call escalation shape the
BLAgent result argues for: locate cheaply on signatures, then expand only the
handful of symbols the evidence points at. ``find_symbol`` and the map are
the cheap half; ``expand_symbols`` is the escalation, capped at ten bodies so
it stays an escalation rather than a second way to read the repository.

The expansion is fair when it cannot be complete. A batch of ten bodies can
exceed the output ceiling on its first symbol, and the honest failure is not
"here is symbol one, the ceiling ate the rest" -- a caller who asked about ten
symbols learns nothing from one of them and cannot tell which nine went
missing. So the budget is spent max-min fair: bodies small enough to fit an
equal share are kept whole, their leftover goes back to the pool, and the
bodies still over budget are cut to head lines with the count they were cut
at. Every requested id comes back with its location, its signature and at
least the first lines of its body, every cut is marked on the card that was
cut, and a summary line names how many were shortened and why.

``find_referencing_symbols`` is the asymmetric half of navigation. Callees
come free from reading a body; callers do not, and they are what an error-path
review or a blast-radius question actually needs. Each reference is attributed
to the symbol whose span contains it, so the answer reads as "these functions
call it" rather than as a list of line numbers.

``shared_callers`` is the adjacency pass behind the DRY question: symbols that
the *same* callers also call. A helper that four of your caller's callers
already use is the "we already have a utility for this" signal. The listing is
bounded by the same ``limit`` as the references, ranked with production-defined
candidates ahead of test-defined ones, and each shared caller's vote is damped
by that caller's own fan-out -- a function that calls half the repository says
almost nothing about which of its callees belong together.

Fan-in rows carry an evidence tier from :mod:`agentless_mcp.core.resolve`. The
over-reporting is deliberate and unchanged -- every name match still appears --
but each file's group says whether the file imports the target, defines the
name itself, matched the repository's only definition, or matched nothing but
the spelling, so a reader can weigh the rows instead of trusting them equally.
"""

from collections.abc import Iterable
from dataclasses import dataclass, field, replace
from typing import Any

from agentless_mcp.application import render
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core import graph, refs, resolve
from agentless_mcp.core.cache import effective_source
from agentless_mcp.core.extractor import Ref, TreeSitterExtractor
from agentless_mcp.core.slices import line_prefix
from agentless_mcp.core.symbols import (
    ASTSymbol,
    SymbolKind,
    id_qualname,
    parse_stable_id,
    qualname,
    rationale_stable_id,
    symbol_stable_id,
)
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util import bounds
from agentless_mcp.util.errors import LanguageUnavailable, SecurityRefusal
from agentless_mcp.util.fslimits import contained_path, read_bounded
from agentless_mcp.util.tokens import TokenCounter

DEFAULT_FIND_LIMIT = 20
DEFAULT_REFS_LIMIT = 50
DEFAULT_EXPAND_LIMIT = 10

# What the rendered cards of one expansion may cost. Under the envelope's
# 16k-token ceiling by a margin that covers the receipt, the banner, and the
# few percent JSON escaping adds to the same bodies -- because the service
# budget only does its job if it binds *before* the ceiling does. A batch
# trimmed by the ceiling loses whole symbols; a batch trimmed here loses the
# tails of the longest ones.
EXPAND_BUDGET_TOKENS = 12_000

# How many cards one call may seat at all, however small each one is cut. A
# separate bound from the budget, and deliberately a constant rather than
# something derived from it: the budget governs how the *bodies* are cut, this
# governs how many card *headers* the response can carry, and the headers are
# charged against the envelope's 16k ceiling, which no caller-supplied budget
# moves.
#
# Sized against the JSON form, not the text one. A card's eight JSON keys cost
# roughly what its whole text rendering does (measured 2026-08-19 on a real
# 60-card expansion: 8.1k text tokens, 11.3k JSON), so a seat count read off
# the text render lets through more cards than the ceiling can then carry --
# and the envelope trims that overflow by dropping whole symbols, which is
# precisely the failure the fair split exists to prevent. 40 is four times the
# documented per-call limit and was verified to leave the wrapped JSON clear
# of the ceiling at 40, 60 and 192 requested ids.
EXPAND_MAX_SEATS = 40

# Room kept back on each shortened card for the marker that says it was
# shortened, so announcing the cut cannot be what pushes a card past its
# share.
_TRUNCATION_MARKER_TOKENS = 32


@dataclass(frozen=True)
class FindResult:
    """Symbol matches for one query, already capped at the limit.

    ``cards`` is the listing rather than a bare tuple so that the count behind
    the limit travels with the rows into the renderer. It iterates and indexes
    as the tuple did.

    ``skipped`` is what the scan could not read -- files over the size cap, in
    a language whose grammar is not warmed, or unreadable. It travels with the
    matches because "no matching symbols" over a partial scan is a different
    answer from the same words over a complete one.
    """

    query: str
    cards: render.CardListing
    skipped: tuple[refs.SkippedFile, ...] = ()

    @property
    def total(self) -> int:
        """How many symbols matched before the limit was applied."""
        return self.cards.total

    @property
    def limit(self) -> int:
        """The limit this lookup was answered under."""
        return self.cards.limit

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this result."""
        return {
            "query": self.query,
            **self.cards.as_dict(),
            "skipped": [{"path": entry.path, "reason": entry.reason} for entry in self.skipped],
        }


def _check_limit(limit: int) -> None:
    """Refuse a limit that cannot bound a listing.

    The reasoning that used to live here now lives in
    :func:`agentless_mcp.util.bounds.at_least`, where ``GraphService`` and
    both front doors can reach it. This module was the only layer that had
    the rule, which is why every listing ``GraphService`` owns accepted a
    limit of zero and answered with an empty list.
    """
    bounds.at_least(limit, 1, "limit")


@dataclass(frozen=True)
class ExpandResult:
    """Full bodies for the requested stable ids, plus the ids that missed."""

    cards: tuple[render.SymbolCard, ...]
    unresolved: tuple[tuple[str, str], ...]
    budget: int = EXPAND_BUDGET_TOKENS

    @property
    def shortened(self) -> int:
        """How many of the returned bodies were cut to fit the budget."""
        return sum(1 for card in self.cards if card.body_shown < card.body_total)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this expansion."""
        return {
            "budget_tokens": self.budget,
            "shortened": self.shortened,
            "symbols": [card.as_dict() for card in self.cards],
            "unresolved": [
                {"stable_id": entry, "reason": reason} for entry, reason in self.unresolved
            ],
        }


@dataclass(frozen=True)
class RefsResult:
    """Fan-in for one target: grouped references and optional adjacency.

    ``groups`` is the listing rather than a bare tuple, for the reason its
    docstring gives: the renderer is handed nothing else, and a fan-in that
    cannot say what it left out is read as a complete caller set.
    """

    target: str
    groups: render.RefListing
    shared: render.SharedCallerListing = field(default_factory=render.SharedCallerListing)

    @property
    def total(self) -> int:
        """How many reference sites were found before the limit was applied."""
        return self.groups.total

    @property
    def limit(self) -> int:
        """The limit this fan-in was answered under."""
        return self.groups.limit

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this result."""
        return {
            "target": self.target,
            **self.groups.as_dict(),
            "shared_callers": self.shared.as_dict(),
        }


class SymbolService:
    """Finds, expands and traces symbols. Holds no per-repository state."""

    def __init__(self, extractor: TreeSitterExtractor, counter: TokenCounter) -> None:
        self._extractor = extractor
        self._counter = counter

    def find_symbol(
        self,
        ctx: RepoContext,
        query: str,
        *,
        kind: str | None = None,
        limit: int = DEFAULT_FIND_LIMIT,
    ) -> FindResult:
        """Match a substring or a qualified name across the repository."""
        _check_limit(limit)
        scan = refs.scan_repo(ctx.root, self._extractor, source=ctx.symbols)
        needle = query.lower()

        matches = [
            (facts, symbol)
            for facts in scan.files
            for symbol in facts.symbols
            if _matches(symbol, needle, kind)
        ]
        matches.sort(key=lambda pair: (*_match_rank(pair[1], needle), pair[0].path))

        cards = tuple(symbol_card(symbol) for _, symbol in matches[:limit])
        listing = render.CardListing(rows=cards, total=len(matches), limit=limit)
        return FindResult(query=query, cards=listing, skipped=scan.skipped)

    def expand_symbols(
        self,
        ctx: RepoContext,
        stable_ids: list[str],
        *,
        limit: int = DEFAULT_EXPAND_LIMIT,
        budget: int = EXPAND_BUDGET_TOKENS,
        seats: int = EXPAND_MAX_SEATS,
    ) -> ExpandResult:
        """Return line-numbered bodies for the named stable ids, fairly budgeted."""
        _check_limit(limit)
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

        if len(cards) > seats:
            reason = MESSAGES.expand_no_room.format(requested=len(cards), seats=seats)
            unresolved.extend((card.stable_id, reason) for card in cards[seats:])
            cards = cards[:seats]

        return ExpandResult(
            cards=self._fit_bodies(cards, budget),
            unresolved=tuple(unresolved),
            budget=budget,
        )

    def find_referencing_symbols(
        self,
        ctx: RepoContext,
        target: str,
        *,
        limit: int = DEFAULT_REFS_LIMIT,
        shared_callers: bool = False,
    ) -> RefsResult:
        """Return the sites that reference ``target``, grouped by file."""
        _check_limit(limit)
        scan = refs.scan_repo(ctx.root, self._extractor, source=ctx.symbols)
        index = refs.build_ref_index(scan)
        by_path = scan.by_path()

        definitions = list(refs.definitions_for(index, target))
        sites = _dedupe(
            site for definition in definitions for site in refs.references_to(index, definition)
        )

        resolver = resolve.build_resolver(scan, index)
        target_ids = {symbol_stable_id(definition.symbol) for definition in definitions}
        groups = render.RefListing(
            rows=_group_sites(sites[:limit], by_path, resolver, target_ids),
            total=len(sites),
            limit=limit,
            files=len({site.path for site in sites}),
        )
        ranked = (
            _shared_callers(sites, definitions, index, by_path, ctx.config.stoplist)
            if shared_callers
            else ()
        )
        shared = render.SharedCallerListing(rows=ranked[:limit], total=len(ranked), limit=limit)
        return RefsResult(target=target, groups=groups, shared=shared)

    def _fit_bodies(
        self, cards: list[render.SymbolCard], budget: int
    ) -> tuple[render.SymbolCard, ...]:
        """Spend ``budget`` across the cards max-min fair, cutting only what must be.

        The allocation is the classic water-filling one, and it is what makes
        the degradation fair rather than positional. Every round divides what
        is left of the budget equally among the cards still competing; the
        cards that already fit their share are settled at full length and give
        their unspent tokens back; the rest go round again on a larger share.
        The loop ends when a round settles nobody, and every card still
        competing then gets exactly the same allowance -- so a thousand-line
        class and a five-line method are cut to the same size, and no card is
        cut at all while another is still whole and larger.
        """
        if not cards:
            return ()

        costs = {
            index: self._counter.count(render.render_symbol_cards([card]))
            for index, card in enumerate(cards)
        }
        pending = set(costs)
        remaining = budget

        while pending:
            share = remaining // len(pending)
            settled = {index for index in pending if costs[index] <= share}
            if not settled:
                break
            remaining -= sum(costs[index] for index in settled)
            pending -= settled

        if not pending:
            return tuple(cards)

        share = max(0, remaining // len(pending))
        return tuple(
            self._shorten(card, share) if index in pending else card
            for index, card in enumerate(cards)
        )

    def _shorten(self, card: render.SymbolCard, share: int) -> render.SymbolCard:
        """Cut one card's body to its share, marked with what was left out.

        At least one body line survives whatever the share is: a card with a
        location and no source at all is the "an id that got no content"
        failure in a smaller form, and one line is what makes the card still
        point somewhere.
        """
        lines = card.body.split("\n")
        header = self._counter.count(render.render_symbol_cards([replace(card, body="")]))
        room = share - header - _TRUNCATION_MARKER_TOKENS

        # Binary search over the line count rather than a walk, so a
        # thousand-line body costs ten counts and not a thousand -- the
        # counter is a protocol and the installed one may be a real tokenizer.
        low, high = 1, len(lines)
        while low < high:
            middle = (low + high + 1) // 2
            if self._counter.count("\n".join(lines[:middle])) <= room:
                low = middle
            else:
                high = middle - 1
        kept = low

        marker = MESSAGES.expand_body_truncated.format(shown=kept, total=len(lines))
        return replace(
            card,
            body="\n".join([*lines[:kept], marker]),
            body_shown=kept,
            body_total=len(lines),
        )

    def _expand_one(self, ctx: RepoContext, raw: str) -> tuple[render.SymbolCard | None, str]:
        """Resolve one stable id to a card carrying the symbol's whole body."""
        try:
            parsed = parse_stable_id(raw)
        except ValueError as exc:
            return None, str(exc)

        source, symbols, reason = self._parse_one(ctx, parsed.path)
        if symbols is None:
            return None, reason

        match = next((symbol for symbol in symbols if id_qualname(symbol) == parsed.qualname), None)
        if match is None:
            return None, f"{parsed.path} no longer defines {parsed.qualname}"

        lines = source.split("\n")
        start = match.line_number
        end = min(len(lines), match.end_line_number or match.line_number)
        body = "\n".join(
            f"{line_prefix(number)}{lines[number - 1]}" for number in range(start, end + 1)
        )
        return symbol_card(match, body=body), ""

    def _parse_one(self, ctx: RepoContext, path: str) -> tuple[str, list[ASTSymbol] | None, str]:
        """Read and parse one file, degrading that file alone when it cannot be.

        Every way one file can fail -- a path that escapes the repository, an
        unreadable or oversized file, a suffix with no grammar, a grammar that
        was never warmed -- comes back as a reason for the id that asked for
        it. That is the convention :func:`agentless_mcp.core.refs._parse_one`
        sets, and an exception here instead would discard every card the batch
        had already built while leaving the caller unable to tell which id
        poisoned the call.
        """
        try:
            # The id came from a caller, so its path is foreign data even
            # though this package generated the id in the first place.
            absolute = contained_path(ctx.root, path)
        except SecurityRefusal as exc:
            return "", None, str(exc)

        read = read_bounded(absolute)
        if read.text is None:
            return "", None, f"{path}: {read.skipped}"

        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(absolute.suffix)
        if language is None:
            return "", None, f"{path}: no grammar for this file type"

        try:
            symbols = effective_source(ctx.symbols, self._extractor).symbols_for(
                read.text, language, path
            )
        except LanguageUnavailable as exc:
            return "", None, f"{path}: {exc}"
        return read.text, list(symbols), ""


def render_expansion(result: ExpandResult) -> str:
    """Render an expansion: the cards, what was shortened, and what missed.

    One home for the three pieces because both adapters print all three, and
    an adapter that printed the cards without the shortening summary would be
    the silent-truncation defect back in a different file.
    """
    blocks = [render.render_symbol_cards(result.cards)]
    if result.shortened:
        blocks.append(
            MESSAGES.expand_batch_shortened.format(
                shortened=result.shortened, total=len(result.cards), budget=result.budget
            )
        )
    blocks.extend(f"unresolved: {entry} -- {reason}" for entry, reason in result.unresolved)
    return "\n".join(blocks)


def render_find(result: FindResult) -> str:
    """Render a lookup: any skipped-file warning, then the cards.

    One home for both pieces, for the same reason :func:`render_expansion` is
    one: an adapter that printed the cards without the warning would hand
    "no matching symbols" to a reader whose file was never scanned. The
    warning comes first because it changes how the listing below it -- and
    especially an empty one -- must be read.
    """
    body = render.render_symbol_cards(result.cards)
    warning = render.render_skipped_files(result.skipped)
    if not warning:
        return body
    return warning + "\n\n" + body


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


def symbol_card(symbol: ASTSymbol, body: str = "") -> render.SymbolCard:
    """Build the incident card for one symbol.

    Public because every view that shows a symbol shows the same card: the
    lookup results here, and the definition site an explanation opens with.

    A card starts out whole: shown and total both count the body handed in, so
    only :meth:`SymbolService._shorten` can ever make them disagree.
    """
    lines = len(body.split("\n")) if body else 0
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
        body_shown=lines,
        body_total=lines,
    )


def rationale_nodes(symbol: ASTSymbol) -> tuple[render.RationaleNode, ...]:
    """Build the rationale nodes linked to one symbol."""
    parent_id = symbol_stable_id(symbol)
    return tuple(
        render.RationaleNode(
            stable_id=rationale_stable_id(symbol, rationale),
            parent_id=parent_id,
            line=rationale.line_number,
            kind=rationale.kind,
            text=rationale.text,
            citations=rationale.citations,
        )
        for rationale in symbol.rationales
    )


def _dedupe(sites: Iterable[Ref]) -> list[Ref]:
    """Collapse duplicate sites and order them by file then line."""
    unique = {(site.path, site.line, site.name): site for site in sites}
    return [unique[key] for key in sorted(unique)]


def _group_sites(
    sites: list[Ref],
    by_path: dict[str, refs.FileFacts],
    resolver: resolve.Resolver,
    target_ids: set[str],
) -> tuple[render.RefGroup, ...]:
    """Group reference sites by file, attributing and tiering each group."""
    grouped: dict[str, list[render.RefSite]] = {}
    names: dict[str, str] = {}
    for site in sites:
        facts = by_path.get(site.path)
        symbol = refs.enclosing_symbol(facts, site.line) if facts else None
        names.setdefault(site.path, site.name)
        grouped.setdefault(site.path, []).append(
            render.RefSite(
                line=site.line,
                enclosing=qualname(symbol) if symbol else render.MODULE_LEVEL,
                stable_id=symbol_stable_id(symbol) if symbol else None,
            )
        )

    return tuple(
        _ref_group(path, tuple(grouped[path]), names[path], resolver, target_ids)
        for path in sorted(grouped)
    )


def _ref_group(
    path: str,
    sites: tuple[render.RefSite, ...],
    name: str,
    resolver: resolve.Resolver,
    target_ids: set[str],
) -> render.RefGroup:
    """Label one file's references with the evidence tier behind them.

    The tier is the tier at which *this file* resolves the name -- and it is
    reported only when the resolution actually lands on the target. A file
    that defines its own ``quote`` resolves the name to its own definition, so
    its rows are name-only evidence about somebody else's ``quote`` no matter
    how strong the local binding is; labelling them ``name-only-ambiguous`` is
    what tells a reader that the shadowing happened.
    """
    resolution = resolver.resolve(name, path)
    tier = resolve.Tier.AMBIGUOUS
    if resolution is not None:
        resolved_ids = {symbol_stable_id(entry.symbol) for entry in resolution.candidates}
        if resolved_ids & target_ids:
            tier = resolution.tier
    return render.RefGroup(path=path, sites=sites, tier=tier.value, tier_label=tier.label)


@dataclass
class _Adjacency:
    """One candidate symbol and the target's callers that also reference it.

    Each caller is stored with its own fan-out -- how many distinct names its
    body references -- because that is what its vote is damped by when the
    row is scored.
    """

    definition: refs.Definition
    callers: dict[str, tuple[ASTSymbol, int]] = field(
        default_factory=dict[str, tuple[ASTSymbol, int]]
    )

    def add(self, caller: ASTSymbol, out_degree: int) -> None:
        """Record one shared caller and its fan-out, keyed so it counts once."""
        self.callers[symbol_stable_id(caller)] = (caller, out_degree)

    def row(
        self, stable: str, index: refs.RefIndex, stoplist: frozenset[str]
    ) -> render.SharedCaller:
        """Render this candidate as a ranked adjacency row."""
        symbol = self.definition.symbol

        # Files vote, not sites, and each file votes with its most focused
        # caller: a caller's vote is one over the same log damping the
        # candidate's name spread gets, so a builder that references every
        # name in the repository contributes almost nothing while a two-line
        # caller contributes nearly a full vote.
        weight_by_file: dict[str, float] = {}
        for caller, out_degree in self.callers.values():
            weight = 1.0 / graph.common_name_damping(out_degree)
            path = caller.module_path
            weight_by_file[path] = max(weight_by_file.get(path, 0.0), weight)

        spread = index.files_referencing.get(symbol.name, 1)
        score = (
            sum(weight_by_file.values())
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
                for caller, _ in self.callers.values()
            ),
            key=lambda caller: (caller.path, caller.line),
        )
        return render.SharedCaller(
            stable_id=stable,
            path=self.definition.path,
            line=symbol.line_number,
            overlap=len(self.callers),
            shared_files=len(weight_by_file),
            score=score,
            callers=tuple(callers),
            in_tests=_defined_in_tests(self.definition.path),
        )


def _defined_in_tests(path: str) -> bool:
    """True when ``path`` sits under a test tree.

    A path heuristic, because the scan carries no structural notion of a test
    tree -- and deliberately scoped to whole path segments named ``test`` or
    ``tests`` plus ``conftest`` modules, so that ``latest/`` or ``contest.py``
    cannot match. Paths are repository-relative with forward slashes.
    """
    segments = path.split("/")
    if any(segment in ("test", "tests") for segment in segments[:-1]):
        return True
    return segments[-1].rsplit(".", 1)[0] == "conftest"


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

    Three corrections make that ranking usable on a real repository, and each
    is the treatment :mod:`agentless_mcp.core.graph` already applies to the
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
    * **Promiscuous callers are damped.** A caller that references half the
      repository shares callers with everything it touches, and carries
      almost no similarity information doing it. Each caller's vote is one
      over the same log damping applied to its own fan-out, so a
      characterization-test builder that calls every method in the codebase
      cannot flood the ranking with ties.
    * **Locally bound names are not references.** A parameter's name spells
      its own binding, so counting it would make a caller "share" every
      symbol in the repository that happens to be spelled like one of its
      arguments. ``core.refs`` keeps those sites deliberately -- fan-in lists
      every place a name is spelled -- and leaves the filtering to each
      consumer that turns a site into a *relationship*, which this is.

    Production-defined candidates rank ahead of every test-defined one
    whatever the scores say, because the question is "does a production
    utility for this already exist"; the test rows are still listed --
    hiding them would misreport the repository -- just after, and marked.
    Every ranked row is returned; the caller's limit decides how many the
    listing keeps and counts the rest.
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
        names = {
            ref.name
            for ref in facts.refs
            if caller.line_number <= ref.line <= end and ref.is_reference
        }
        for name in sorted(names):
            for definition in index.definitions.get(name, ()):
                candidate = symbol_stable_id(definition.symbol)
                if candidate in target_ids or candidate == caller_id:
                    continue
                shared.setdefault(candidate, _Adjacency(definition)).add(caller, len(names))

    rows = [
        entry.row(candidate, index, stoplist)
        for candidate, entry in shared.items()
        if len(entry.callers) > 1
    ]
    rows.sort(key=lambda row: (row.in_tests, -row.score, -row.shared_files, row.stable_id))
    return tuple(rows)


def kind_names() -> tuple[str, ...]:
    """Return the symbol kinds a caller may filter on."""
    return tuple(kind.value for kind in SymbolKind)
