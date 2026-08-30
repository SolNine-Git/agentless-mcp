"""The repository map: rank files, pick symbols, pack to a token budget.

The pipeline is one pass and no state: walk, extract, collect references,
build the file graph, rank it with personalized PageRank, spread each file's
rank across the symbols inside it, then pack as many symbols as the budget
allows and render them code-shaped.

Three defaults come straight from the research the plan records, and each is
a number rather than a knob-with-no-answer:

* **Function granularity, ten files.** Function-level localization beats both
  file-level and line-level (45.6% / 42.6% / 43.6%), and the file stage is
  capped at ten because a longer list stops being a funnel.
* **A budget that scales with the repository.** ``auto`` is the candidate
  set's own size divided by six, clamped to 2k-8k tokens as the configured
  :class:`~agentless_mcp.util.tokens.TokenCounter` counts them -- see that
  module for what the default unit is worth against a real tokenizer. Roughly 6x
  compression measurably *raises* resolve rate over full context, while 22-50x
  is worse than either -- so the objective is minimal sufficient context, not
  the highest ratio available.
* **Seeds take the whole teleport mass.** ``--focus`` is not a filter, it is
  the personalization vector: the files a caller names pull rank toward
  whatever they depend on, which is how a map answers "what else does this
  touch" rather than "what is big".

Packing is a binary search over the number of included symbols rather than a
greedy fill, so the answer is the largest prefix of the score ordering that
fits -- deterministic, and independent of the order files were walked in.
"""

import re
import time
from collections.abc import Collection, Mapping, Sequence, Set
from dataclasses import dataclass, field, replace
from pathlib import PurePosixPath
from typing import Any

from agentless_mcp.application import render
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import (
    EXPAND_MAX_SEATS,
    ExpandResult,
    SymbolService,
    is_test_path,
    rationale_nodes,
    render_expansion,
    unresolved_lines,
)
from agentless_mcp.core import projectconfig, refs
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.graph import (
    RefGraph,
    build_graph,
    common_name_damping,
    flood,
    name_multiplier,
    personalized_pagerank,
    rank_order,
)
from agentless_mcp.core.projectconfig import MAX_MAX_FILES, MIN_MAX_FILES
from agentless_mcp.core.symbols import (
    ASTSymbol,
    id_qualname,
    language_prefix,
    qualname,
    symbol_stable_id,
)
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util import bounds
from agentless_mcp.util.errors import OperationFailed
from agentless_mcp.util.tokens import TokenCounter

DEFAULT_MAX_FILES = 10

# How many test files the companion section lists before cutting to a count.
# Small on purpose: the section sits outside the token budget the ranked files
# are packed into, so every row it adds is a row the caller did not ask for.
# Five is enough to name the suites that exercise a ten-file map and short
# enough that a repository with a thousand tests cannot turn the map into one.
DEFAULT_MAX_TEST_FILES = 5

# How far the companion walk goes out from the seeded-or-chosen files. One hop
# is the test that exercises its subject directly, which is what the section is
# for; two is the test that reaches it through a helper or a wrapper. Past that
# the relationship is thinner than the row costs.
TEST_COMPANION_DEPTH = 2

GRANULARITY_FUNCTION = "function"
GRANULARITY_FILE = "file"
GRANULARITY_BODY = "body"
# The wire surface's choices. Deliberately wider than
# ``projectconfig.GRANULARITIES``, which stays ('function', 'file'): a config
# default of 'body' would make the expensive view every call's silent default,
# so the config door never gains it. A test pins the divergence -- do not
# merge the two tuples.
GRANULARITIES = (GRANULARITY_FUNCTION, GRANULARITY_FILE, GRANULARITY_BODY)

# How many whole bodies one budget is worth: expand's own shipped ratio,
# EXPAND_BUDGET_TOKENS / EXPAND_MAX_SEATS = 12000 / 40 = 300 tokens a seat.
# The seat count bounds how many ids a body map expands; the water-filling
# then spends the actual budget fairly across them.
BODY_TOKENS_PER_SEAT = 300

# "auto": aim at ~6x compression of the candidate set, then refuse to go
# below a map that could not say anything or above one that stops being a map.
AUTO_BUDGET_DIVISOR = 6
AUTO_BUDGET_MIN = 2_000
AUTO_BUDGET_MAX = 8_000

# Past this many rendered tokens the auto-size estimate is already clamped to
# AUTO_BUDGET_MAX, so whatever the rest of the candidate set renders to cannot
# move it.
AUTO_BUDGET_CEILING = AUTO_BUDGET_MAX * AUTO_BUDGET_DIVISOR

# Where the auto-size probe starts, and how fast it grows. Measured on this
# repository, 2,000 candidates already render to about 60,000 tokens, past
# the ceiling above -- so the first step settles the clamp for any set large
# enough for the waste to matter, and a smaller set costs exactly one render,
# the same as before the probe existed.
AUTO_BUDGET_PROBE = 2_000
AUTO_BUDGET_PROBE_GROWTH = 4


@dataclass(frozen=True)
class MapRequest:
    """What a caller asked the map for, before the defaults are filled in.

    ``None`` means "the caller did not say", which is a different fact from
    any particular value and the one the precedence rule needs: an explicit
    argument beats the repository's ``.agentless-mcp.json``, which beats the
    built-in default. Resolving that here rather than in each adapter is what
    keeps the CLI and the MCP server from answering the same question two
    ways.

    ``budget`` is the exception: ``None`` means auto-size, which is a real
    answer rather than an absent one, so the adapters resolve it before
    building the request.
    """

    focus: tuple[str, ...] = ()
    budget: int | None = None
    max_files: int | None = None
    granularity: str | None = None


@dataclass(frozen=True)
class MapResult:
    """A rendered-ready map plus the numbers that explain its shape."""

    files: tuple[render.MapFile, ...]
    budget: int
    included: int
    candidates: int
    seeds: tuple[str, ...]
    skipped: tuple[refs.SkippedFile, ...]
    unresolved_seeds: tuple[str, ...] = ()
    # How many files the ranking produced before --max-files cut it. Carried
    # so an empty map can say which of three things happened rather than
    # asserting the one an agent stops on: nothing parsed, the file limit
    # kept none, or the token budget fitted none. Added alongside the
    # existing fields rather than replacing one, because this is a shipped
    # JSON shape.
    ranked: int = 0
    # What the rendered map actually costs, in the same tokens ``budget`` is
    # spelled in. The packing search bounds the *symbols* it includes, but a
    # ranked file is listed whether or not any of its symbols fit, so the file
    # headers sit outside the search and the budget is a bound on the body
    # alone. Carried so the renderer can say when the headers went past it,
    # instead of reporting a budget as honoured when it was not.
    rendered: int = 0
    # Whether the power iteration behind the ranking settled. The whole map is
    # an ordering, so a ranking that ran out of iterations makes the order the
    # reader is about to trust a partial answer, and silence about that is the
    # failure this package exists to prevent.
    rank_converged: bool = True
    # The included symbols' stable ids in the packing's own score order --
    # what `build_body_map` expands when the seats are fewer than the ids.
    # `files` re-sorts each file's entries by line number for reading, so
    # this field is the only record of which of them scored highest. Not in
    # the JSON form: it restates ids the files already carry, in an order
    # only the body composition consumes.
    expand_order: tuple[str, ...] = ()
    # The focus-named symbols' stable ids, in the ranking's file order --
    # what `build_body_map` seats before anything from `expand_order`. The
    # score behind that order is inbound-name centrality, which knows nothing
    # of the focus, so without this field a caller who named a symbol got the
    # file's most self-referential boilerplate expanded and still needed the
    # second round trip the body view exists to remove. Independent of the
    # packing on purpose: a tight budget can cut the named symbol from
    # `expand_order`, and the seat must survive that. Not in the JSON form,
    # for `expand_order`'s reason.
    focus_order: tuple[str, ...] = ()
    # The test files that exercise the ranked or seeded files, listed outside
    # the budget the ranked files are packed into. Edges run referrer to
    # definer, so a test file has no inbound weight and the ranking that
    # scores inbound weight cannot place it however relevant it is; this is
    # the only route a test has into a map. Added as a key rather than folded
    # into an existing one, because this JSON shape is shipped.
    test_companions: render.TestCompanionListing = field(
        default_factory=render.TestCompanionListing
    )

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this map."""
        return {
            "budget_tokens": self.budget,
            "symbols_included": self.included,
            "symbols_available": self.candidates,
            "files_ranked": self.ranked,
            "rendered_tokens": self.rendered,
            "rank_converged": self.rank_converged,
            "seeds": list(self.seeds),
            "unresolved_seeds": list(self.unresolved_seeds),
            "files": [map_file.as_dict() for map_file in self.files],
            "skipped": [{"path": entry.path, "reason": entry.reason} for entry in self.skipped],
            "test_companions": self.test_companions.as_dict(),
        }


@dataclass(frozen=True)
class _Candidate:
    """One symbol competing for a place in the map."""

    score: float
    path: str
    symbol: ASTSymbol


@dataclass(frozen=True)
class _Packing:
    """Everything the budget search renders against, in one value.

    The five fields do not vary independently -- one ranking produces all of
    them, and :func:`_group`, :meth:`MapService._auto_budget` and
    :meth:`MapService._pack` each need every one -- so they travel together
    rather than as a parameter list three functions repeat.

    ``eligible`` is what competes for the budget: the scored symbols in files
    the focus reached. ``candidates`` is the whole scored set and is read only
    for the per-file totals, which is what lets an unreached file state how
    much it holds without any of it being rendered. ``paths`` is the ranked
    file order, ``rank`` the number each row prints, and ``reached`` the set
    that decides which of the two omission lines a file gets.
    """

    eligible: list[_Candidate]
    candidates: list[_Candidate]
    paths: Sequence[str]
    rank: Mapping[str, float]
    reached: Set[str]


class MapService:
    """Builds repository maps. Holds no per-repository state."""

    def __init__(self, extractor: TreeSitterExtractor, counter: TokenCounter) -> None:
        self._extractor = extractor
        self._counter = counter

    def build(self, ctx: RepoContext, request: MapRequest) -> MapResult:
        """Rank, score, pack and return the map for one repository."""
        max_files = projectconfig.resolve(
            request.max_files, ctx.config.max_files, DEFAULT_MAX_FILES
        )
        # The declared contract is projectconfig's MIN/MAX_MAX_FILES, and it
        # was enforced for a config file and on the MCP wire but not here, so
        # the CLI -- which declares --max-files as a bare type=int -- honoured
        # anything. Enforced in the service means both doors inherit it.
        bounds.within(max_files, MIN_MAX_FILES, MAX_MAX_FILES, "max_files")
        # ``budget`` had the same shape and one door fewer: projectconfig
        # enforced MIN_BUDGET..MAX_BUDGET for a `.agentless-mcp.json` entry and
        # the MCP schema published it, while `--budget` parsed only "a positive
        # integer" and no service checked it at all. `map --budget 1` therefore
        # exited 0 reporting that the budget left room for no symbols, which is
        # a fact about the request dressed as one about the repository.
        #
        # This call is the rule. The other two places the range appears --
        # `projectconfig._bounded_int` for the config file and the
        # `Field(ge=, le=)` on the MCP parameter -- are re-exports of it, kept
        # because each door has to refuse in its own idiom: a config file
        # warns and falls back, and a wire schema has to advertise the range
        # before the call is made. A drift between the three is fixed here and
        # propagated outward, never by editing whichever copy turned up first.
        #
        # ``None`` is the auto-size request rather than a value, so it is not a
        # number to bound.
        if request.budget is not None:
            bounds.within(
                request.budget,
                projectconfig.MIN_BUDGET,
                projectconfig.MAX_BUDGET,
                "budget",
            )
        granularity = projectconfig.resolve(
            request.granularity, ctx.config.granularity, GRANULARITY_FUNCTION
        )
        if granularity == GRANULARITY_BODY:
            # The adapters route 'body' to build_body_map before build is
            # reached. Falling through to the function view here would answer
            # a different question than the one asked, silently.
            message = "granularity 'body' is composed by build_body_map, not by build"
            raise OperationFailed(message)
        scan = refs.scan_repo(ctx.root, self._extractor, source=ctx.symbols)
        index = refs.build_ref_index(scan)
        # The stoplist is a property of the repository rather than of the
        # request: it says which names collide in *this* codebase, which the
        # codebase is better placed to know than the caller.
        graph = build_graph(
            scan,
            index,
            stoplist=ctx.config.stoplist,
            relation_weights=bool(ctx.config.relation_weights),
        )

        seeding = seed_weights(request.focus, scan, index)
        seeds = seeding.weights
        # Tests reach the code they exercise and are never what it is about,
        # so they stay pure sources of rank and are reported below as
        # companions instead. `companions_for` reads the same graph backwards
        # to find them, which is the channel this keeps them in.
        tests = {path for path in graph.nodes if is_test_path(path)}
        ranking = personalized_pagerank(graph, seeds or None, pure_sources=tests)
        rank = ranking.rank
        chosen = rank_order(rank, set(seeds))[: max(0, max_files)]

        # `chosen` is keyed the same way as `by_path` and `rank`: all three
        # come from this one scan, so a missing key is a desynchronisation
        # worth raising on rather than reading as an empty file.
        by_path = scan.by_path()
        # Seeded *and* chosen, not chosen alone: a seed the ranking did not
        # keep is still the file the caller asked about, and the test that
        # exercises it is the answer to the question they asked.
        companions = companions_for(graph, by_path, index, set(seeds) | set(chosen))
        if granularity == GRANULARITY_FILE:
            files = tuple(
                render.MapFile(
                    path=path,
                    rank=rank[path],
                    total=len(by_path[path].symbols),
                    # Every file here has its whole symbol count omitted, so
                    # every one of them takes an omission line -- which makes
                    # this the granularity where the wrong line is offered
                    # most often, not least. Left to the default, a focused
                    # file map told a caller to raise a budget this view does
                    # not have, about a file no budget could reach.
                    reached=path in ranking.support,
                )
                for path in chosen
            )
            files = _with_churn(files, ctx)
            return MapResult(
                files=files,
                budget=0,
                # No symbol competed for a place here, which is a different
                # fact from "none of them fit". Reporting the repository's
                # symbol count made both adapters render `0 of N symbols
                # shown ... raise the budget` under a view that shows no
                # symbols by design and has no budget to raise. Each file's
                # own count is still on its row, as `omitted`.
                included=0,
                candidates=0,
                ranked=len(rank),
                seeds=tuple(sorted(seeds)),
                skipped=scan.skipped,
                unresolved_seeds=seeding.unresolved,
                rendered=self._counter.count(render.render_map(files)),
                rank_converged=ranking.converged,
                test_companions=companions,
            )

        candidates = _score_symbols(chosen, by_path, index, rank, ctx.config.stoplist)
        # Only the files the focus actually reaches compete for the budget.
        # A seeded map teleports to the seeds alone, so a file no reference
        # path connects to them holds nothing but the residue of the uniform
        # vector the iteration started from -- it renders as rank 0.0000 and
        # is not zero, so the test is reachability rather than a threshold on
        # the number. Measured on this repository: an exact-path seed ranked
        # one file and padded the answer with nine unreached ones that took
        # most of the budget.
        #
        # The unreached files are still listed. `_group` gives every ranked
        # file a row with the count of what it holds, and dropping them would
        # be the bounded-view-mistaken-for-complete failure that function
        # exists to prevent -- as well as breaking the top-N contract the
        # tool description publishes. What changes is where the budget goes,
        # not how many files come back.
        eligible = [entry for entry in candidates if entry.path in ranking.support]
        packing = _Packing(
            eligible=eligible,
            candidates=candidates,
            paths=chosen,
            rank=rank,
            reached=ranking.support,
        )
        budget = request.budget if request.budget is not None else self._auto_budget(packing)
        included = self._pack(packing, budget)
        grouped = _with_churn(_group(included, packing), ctx)

        return MapResult(
            files=grouped,
            expand_order=tuple(
                symbol_stable_id(entry.symbol) for entry in packing.eligible[:included]
            ),
            focus_order=_focus_order(seeding.definitions, chosen),
            budget=budget,
            included=included,
            # What competed for the budget, not every symbol under a ranked
            # file. The banner reads "N of M symbols shown (narrow the request
            # or raise the budget for the rest)", and no budget shows a symbol
            # in a file the focus never reached, so counting those in M makes
            # that advice false. Each unreached file still carries its own
            # count on its row. Equal to the whole set on an unfocused map,
            # where the walk reaches everything.
            candidates=len(eligible),
            ranked=len(rank),
            seeds=tuple(sorted(seeds)),
            skipped=scan.skipped,
            unresolved_seeds=seeding.unresolved,
            rendered=self._counter.count(render.render_map(grouped)),
            rank_converged=ranking.converged,
            test_companions=companions,
        )

    def _count_tokens(self, text: str) -> int:
        """Count ``text`` in the same unit every budget here is spelled in."""
        return self._counter.count(text)

    def render_text(self, result: MapResult) -> str:
        """Render a map result as code-shaped text.

        Unresolved seeds and skipped files are named at the top rather than at
        the bottom: both notes change how the ranking below them should be
        read, and a reader who stops at the first interesting filename has to
        have seen them.

        The test companions go at the foot and only when there are any. They
        are an answer to a second question -- what exercises these files --
        so they must not interleave with the ranking, and a repository whose
        map reaches no test spends nothing saying so.
        """
        body = render.render_map(result.files)
        companions = render.render_test_companions(result.test_companions)
        if companions:
            body = f"{body}\n{companions}"
        notes: list[str] = []
        if not result.rank_converged:
            notes.append(
                "the ranking behind this map did not converge, so the order of "
                "the files below is a partial answer"
            )
        if not result.files:
            notes.append(self._why_nothing_ranked(result))
        if result.budget and result.rendered > result.budget:
            notes.append(
                f"this map renders to {result.rendered} tokens against a "
                f"{result.budget}-token budget: every one of the {len(result.files)} "
                "ranked files is listed whatever the budget allows, and the budget "
                "bounds only the symbols shown under them"
            )
        if result.unresolved_seeds:
            listed = ", ".join(result.unresolved_seeds)
            notes.append(MESSAGES.map_unresolved_seeds.format(seeds=listed))
        warning = render.render_skipped_files(result.skipped)
        if warning:
            notes.append(warning)
        if not notes:
            return body
        return "\n".join(notes) + "\n\n" + body

    def _why_nothing_ranked(self, result: MapResult) -> str:
        """Say which of the three empty results this is.

        `render_map` is handed rows and nothing else, so it can only report
        that there are none. This layer holds the candidate count and the
        budget, which is what tells "the repository parsed into no symbols"
        apart from "the budget left room for none of them" -- and the second
        reads as the first to an agent that then stops looking.

        Only the first case now arrives through :meth:`build`: every ranked
        file is listed whatever the packing search decides, so a map over a
        repository that parsed into anything has rows. The other two stay
        because a caller may build a :class:`MapResult` itself and render it,
        and because that is the shape this method is pinned against.
        """
        if not result.ranked:
            return "nothing in this repository parsed into symbols"
        if not result.candidates:
            return f"--max-files kept none of the {result.ranked} ranked files"
        return (
            f"the {result.budget}-token budget left room for none of "
            f"{result.candidates} symbols; raise --budget"
        )

    def _auto_budget(self, packing: "_Packing") -> int:
        """Size the budget from the candidate set, clamped to the useful band.

        Rendered in growing prefixes rather than whole. The estimate is
        clamped into a fixed band, so the moment a prefix renders past
        :data:`AUTO_BUDGET_CEILING` the rest of the set cannot move the answer
        and rendering it is work with a provably known result -- measured at
        49,387 candidates on this repository to arrive at the same 8,000. A
        candidate set under :data:`AUTO_BUDGET_PROBE` still costs exactly one
        render.
        """
        size = AUTO_BUDGET_PROBE
        while True:
            rendered = self._counter.count(render.render_map(_group(size, packing)))
            if rendered > AUTO_BUDGET_CEILING:
                return AUTO_BUDGET_MAX
            if size >= len(packing.eligible):
                estimate = rendered // AUTO_BUDGET_DIVISOR
                return max(AUTO_BUDGET_MIN, min(AUTO_BUDGET_MAX, estimate))
            size *= AUTO_BUDGET_PROBE_GROWTH

    def _pack(self, packing: "_Packing", budget: int) -> int:
        """Return the largest number of eligible symbols whose render fits ``budget``.

        The search is over :attr:`_Packing.eligible` alone.
        :attr:`_Packing.candidates` is the whole scored set and is never
        searched: it is what tells :func:`_group` how many symbols each ranked
        file holds, so the render measured here is the render the caller gets,
        file rows for the unreached files included.
        """
        if not packing.eligible:
            return 0

        low, high = 0, len(packing.eligible)
        while low < high:
            middle = (low + high + 1) // 2
            text = render.render_map(_group(middle, packing))
            if self._counter.count(text) <= budget:
                low = middle
            else:
                high = middle - 1
        return low


def companions_for(
    graph: RefGraph,
    by_path: Mapping[str, refs.FileFacts],
    index: refs.RefIndex,
    targets: Collection[str],
    *,
    limit: int = DEFAULT_MAX_TEST_FILES,
) -> render.TestCompanionListing:
    """Return the test files that exercise ``targets``, best first.

    The map's ranking cannot do this. Edges run referrer to definer, so a test
    file is a pure source: it has no inbound weight, personalized PageRank
    scores inbound weight, and a test therefore never places however directly
    it exercises the file above it. This walks the graph the other way instead.

    **One row per test file.** A suite that touches six of the ranked files
    would otherwise take six of the five rows and starve the specific tests
    that matter, so each file is aggregated into one row naming what it covers.

    **A test qualifies on a name reference, never on an import alone.** An
    import says a module was pulled in; a reference says a name from it was
    used, and only the reference has a line to point at. Two things fall out
    of that and both are wanted. A ``from app import a, b, c`` that exercises
    one of the three counts one, so import fan-out cannot buy coverage -- see
    the ranking below. And a test whose only use of its subject is a method on
    a value built elsewhere is invisible here, because attribute references
    spell no edge; that limit is the graph's, stated rather than papered over.

    **Ranking, in this order.** Flood depth ascending, then the number of
    distinct ``targets`` the file covers descending, then aggregated edge
    weight descending, then path. Coverage leads inside a depth band because
    weight does not survive a language change: a Go ``*_test.go`` sits in the
    same package as its subject, never earns an import edge, and is scored on
    damped name-reference weight alone, so weight-first buries every Go test
    under the Python and TypeScript ones. Counting covered files is neutral --
    three is three in any language. Depth still leads across bands, and path
    last makes the order total, which is what lets a golden pin it.
    """
    wanted = frozenset(targets)
    reach = flood(graph, wanted, backward=True, max_depth=TEST_COMPANION_DEPTH)
    depths = {row.path: row.depth for row in reach.reached}

    # Everything a file at one depth could have come through: the targets, plus
    # whatever the walk placed strictly nearer them. A one-hop test references
    # a target directly; a two-hop test references the helper that does, and
    # that helper is the file its span has to point at.
    #
    # Built once per depth rather than once per test file. The set depends only
    # on the depth, so recomputing it per row walked the whole reach set again
    # for every test in it -- quadratic in a reach set the flood bounds at
    # `DEFAULT_MAX_FLOOD_VISITED`, on the hot path of every map. Accumulated as
    # a running union rather than read from one bucket, so the meaning stays
    # "every shallower file" if `TEST_COMPANION_DEPTH` is ever raised past two.
    by_depth: dict[int, frozenset[str]] = {}
    nearer = set(wanted)
    for hops in sorted(set(depths.values())):
        by_depth[hops] = frozenset(nearer)
        nearer.update(other for other, other_hops in depths.items() if other_hops == hops)

    rows: list[render.TestCompanion] = []
    for path, depth in depths.items():
        if not is_test_path(path):
            continue
        closer = by_depth[depth]
        # `depths` is keyed by graph node and the graph's nodes are this
        # scan's files, so a missing key is a desynchronisation to raise on.
        found = _companion_reference(by_path[path], index, closer)
        if found is None:
            continue
        covers, span = found
        rows.append(
            render.TestCompanion(
                path=path,
                start=span[0],
                end=span[1],
                covers=covers,
                depth=depth,
                weight=sum(graph.edges.get((path, other), 0.0) for other in closer),
            )
        )

    rows.sort(
        key=lambda row: (
            row.depth,
            -len(wanted.intersection(row.covers)),
            -row.weight,
            row.path,
        )
    )
    return render.TestCompanionListing(
        rows=tuple(rows[:limit]),
        total=len(rows),
        limit=limit,
        exhausted=reach.exhausted,
    )


def _companion_reference(
    facts: refs.FileFacts,
    index: refs.RefIndex,
    closer: frozenset[str],
) -> tuple[tuple[str, ...], tuple[int, int]] | None:
    """Which of ``closer`` a file references by name, and the one span that does it.

    ``None`` when it references none of them, which is how an import-only edge
    is kept out of the section: there is nothing to point at inside the file.

    The span is the referencing symbol that covers the most of ``closer``, ties
    going to the one with the most sites and then to the first declared. One
    symbol rather than the hull of every referencing symbol, because a test
    file's first and last test functions between them span the whole file --
    and a whole-file span is what the budgeted metrics this section exists for
    already treat as no answer. Where no reference sits inside a symbol at all,
    the span is the first and last line that reference ``closer``.

    Same-file definitions shadow repository-wide name matches here exactly as
    they do in ``build_graph``, so the evidence and the edges agree on which
    names connect two files.
    """
    owners = refs.line_owners(facts)
    referenced: set[str] = set()
    hits: dict[tuple[int, int], set[str]] = {}
    sites: dict[tuple[int, int], int] = {}
    loose: list[int] = []

    for ref in facts.refs:
        if not ref.is_reference:
            continue
        defining = index.defining_paths(ref.name)
        if facts.path in defining:
            continue
        reached = closer.intersection(defining)
        if not reached:
            continue
        referenced |= reached
        owner = owners.get(ref.line)
        if owner is None:
            loose.append(ref.line)
            continue
        span = (owner.line_number, refs.span_end(owner))
        hits.setdefault(span, set()).update(reached)
        sites[span] = sites.get(span, 0) + 1

    if not referenced:
        return None
    if hits:
        best = max(hits, key=lambda span: (len(hits[span]), sites[span], -span[0], -span[1]))
    else:
        best = (min(loose), max(loose))
    return tuple(sorted(referenced)), best


@dataclass(frozen=True)
class Seeding:
    """What a request's ``--focus`` arguments resolved to, and what they did not.

    Both halves travel together because reporting one without the other is
    the defect this type exists to close: a map that answers with an empty
    personalization vector and says nothing about it reads as an unfocused
    map somebody asked for.
    """

    weights: dict[str, float]
    unresolved: tuple[str, ...]
    # The definitions the symbol-shaped entries named -- empty for an entry
    # that resolved as a path, because a path names a file, not a symbol in
    # it. Carried so the body map can seat the symbol the caller asked about;
    # the weights alone cannot say one was named, which is the defect that
    # spent every body seat on a file's most self-referential boilerplate.
    definitions: tuple[refs.Definition, ...] = ()


def seed_weights(
    focus: tuple[str, ...],
    scan: refs.RepoScan,
    index: refs.RefIndex,
) -> Seeding:
    """Turn ``--focus`` entries into personalization weights over files.

    A focus entry is a path when it names one and a symbol otherwise, and a
    symbol seeds every file that defines it. Two rules make that fair:

    * **One entry, one vote.** An entry's mass is split across the files it
      resolved to rather than added once per file, so ``--focus Validate``
      naming twenty files cannot drown out ``--focus config/config.go``. The
      seed vector then says "these entries", not "whichever entry happened to
      be spelled with a common name".
    * **Nothing is lost quietly.** An entry matching no file and no symbol is
      still not an error -- the map answers unfocused rather than empty -- but
      it comes back in ``unresolved`` and every renderer says so. A seed
      silently dropped is a caller believing the map was focused when it was
      not.
    """
    known = {facts.path for facts in scan.files}
    weights: dict[str, float] = {}
    unresolved: list[str] = []
    definitions: list[refs.Definition] = []

    for entry in focus:
        cleaned = entry.strip()
        if not cleaned:
            # A blank entry is the absence of an entry, not a seed that
            # failed to resolve -- the same thing an empty ``--focus`` string
            # already means -- so reporting it would name nothing back at the
            # caller. "Nothing is lost quietly" is about entries that spell
            # something the scan could not find.
            continue

        paths, matched = _focus_resolution(cleaned, known, index)
        if not paths:
            unresolved.append(cleaned)
            continue
        definitions.extend(matched)

        share = 1.0 / len(paths)
        for path in paths:
            weights[path] = weights.get(path, 0.0) + share

    return Seeding(
        weights=weights,
        unresolved=tuple(dict.fromkeys(unresolved)),
        definitions=tuple(definitions),
    )


def _focus_order(
    definitions: tuple[refs.Definition, ...], chosen: Sequence[str]
) -> tuple[str, ...]:
    """The focus-named symbols' ids, in the ranking's file order then line.

    Filtered to the chosen files because the body map can only seat what its
    file rows show: a focus definition in a file the ranking cut is the file
    limit's decision, and a seat order that overturned it would expand a body
    under no row. Deduplicated because two entries can name one definition,
    and a repeated id would burn a seat restating a body.
    """
    position = {path: index for index, path in enumerate(chosen)}
    ranked = sorted(
        (definition for definition in definitions if definition.path in position),
        key=lambda definition: (position[definition.path], definition.symbol.line_number),
    )
    return tuple(dict.fromkeys(symbol_stable_id(definition.symbol) for definition in ranked))


def focus_paths(entry: str, known: set[str], index: refs.RefIndex) -> list[str]:
    """Resolve one focus entry to the files it names.

    Public because "a file path, a path suffix, or a symbol name" is what
    *every* view means by a focus argument, and the diagram takes one too. Two
    views resolving the same word two ways would be a defect a reader could
    only find by comparing outputs.

    Five shapes, tried in order of how specific they are:

    1. A repository-relative path, exactly as the scan spells it.
    2. A path suffix -- ``config.go`` for ``config/config.go``.
    3. A module stem -- the same suffix rule with the extension stripped, so
       ``envelope`` matches ``application/envelope.py`` exactly as typing
       ``envelope.py`` would, whatever the extension turns out to be. The
       most natural spelling of a module wins as a path, which also means a
       stem that names a file beats a symbol coincidentally sharing the name.
    4. A qualified symbol name: ``ServerInfo.Validate``, or the qualified
       half of a whole stable id. Narrowing to the definitions that really
       carry that owner is what keeps a receiver-qualified method from
       seeding every file defining any ``Validate``.
    5. A bare function, method or type name, matched exactly against the
       extracted symbols the same way ``find_symbol`` matches -- because a
       method name is the most natural seed an issue report yields, and the
       tool description promises symbol names work.

    A path-shaped entry that matches no file stops at step 3. Falling through
    would take the text after its last dot -- a file extension -- and look
    that up as a symbol, which turns a mistyped path into a confident seed on
    an unrelated file.

    An entry the five shapes cannot resolve gets one more chance: the
    spellings a task actually hands a caller -- a traceback frame, a blob
    URL, ``path:line``, an absolute path -- are stripped to the path inside
    them and retried through the path shapes alone (see
    :func:`_normalized_spellings`). Only on a miss, so an entry that resolves
    as spelled today resolves identically; and never through the symbol
    shapes, because a URL segment or an absolute-path component is a path
    fragment, not a name -- ``https://example.com/quote`` naming the symbol
    ``quote`` would be exactly the confident-seed-on-an-unrelated-file defect
    the step-3 stop exists to prevent.
    """
    return _focus_resolution(entry, known, index)[0]


def _focus_resolution(
    entry: str, known: set[str], index: refs.RefIndex
) -> tuple[list[str], tuple[refs.Definition, ...]]:
    """Resolve one entry to files, and to definitions when it named a symbol.

    One function rather than two lookups, so the path-beats-symbol precedence
    :func:`focus_paths` documents cannot drift between the caller that wants
    files (every view's ranking) and the caller that wants the named symbol
    back (the body map's first seat). The definitions half is empty whenever
    the entry resolved as a path, because a path names a file, not a symbol
    in it. A bare name no qualname owns returns every same-named definition,
    so such a focus can seat homonyms across files before any centrality
    seat -- deliberate: the caller named it, and the seat count caps it.
    """
    paths = _path_matches(PurePosixPath(entry).as_posix(), known)
    if paths:
        return paths, ()

    qualified = entry.rpartition("::")[2] or entry
    if "/" not in qualified:
        name = qualified.rpartition(".")[2] or qualified
        defining = [
            definition for definition in index.definitions.get(name, ()) if definition.path in known
        ]
        owned = [definition for definition in defining if qualname(definition.symbol) == qualified]
        matched = owned or defining
        if matched:
            return sorted({definition.path for definition in matched}), tuple(matched)

    for candidate in _normalized_spellings(entry):
        paths = _path_matches(candidate, known)
        if paths:
            return paths, ()
    return [], ()


def _path_matches(normalized: str, known: set[str]) -> list[str]:
    """Resolve one spelling through the three path shapes, most specific first."""
    if normalized in known:
        return [normalized]

    suffix_matches = sorted(path for path in known if path.endswith(f"/{normalized}"))
    if suffix_matches:
        return suffix_matches

    return sorted(path for path in known if _matches_stem(path, normalized))


# A traceback frame: optional indent, `File "<path>", line N`, the rest free.
_TRACEBACK_FRAME = re.compile(r'\s*File "(?P<path>[^"]+)", line \d+')

# A trailing location: `:12`, `:12-40`, or a blob URL's `#L12` fragment.
_LOCATION_SUFFIX = re.compile(r"(?::\d+(?:-\d+)?|#L\d+(?:-L?\d+)?)$")

# A URL scheme prefix, `https://` and friends.
_URL_SCHEME = re.compile(r"^[A-Za-z][A-Za-z0-9+.-]*://")

# How many trailing path components a rescue walk will try. Real spellings
# put the repository-relative path in the last few components; a bound keeps
# a pathological entry from scanning the file set thousands of times.
_MAX_TAIL_COMPONENTS = 32


def _normalized_spellings(entry: str) -> list[str]:
    """The path spellings hiding inside one unresolved focus entry.

    Longest first, deduplicated, never the entry itself: the quoted path of a
    traceback frame, the entry with its URL scheme, query string, fragment and
    trailing ``:line`` stripped, and then every shorter ``/``-tail of that
    path until one names a file -- which is the caller's loop, not this
    function's. The tail walk is what rescues a blob URL or an absolute path
    from another machine: the components that never existed in this
    repository drop away one at a time until what is left is the
    repository-relative spelling.
    """
    frame = _TRACEBACK_FRAME.match(entry)
    if frame:
        core = frame.group("path")
    else:
        core = _URL_SCHEME.sub("", entry)
        if core != entry:
            core = core.split("?", 1)[0].split("#", 1)[0]
    core = _LOCATION_SUFFIX.sub("", core).strip()
    # A Windows spelling: the tail walk splits on the repository's own
    # separator, so a backslash path -- a Windows traceback, an absolute path
    # from a Windows machine -- would otherwise walk nothing. Converted only
    # here, on the miss path: a tracked filename containing a literal
    # backslash resolves as spelled before the rescue ever runs.
    if "\\" in core:
        core = core.replace("\\", "/")

    candidates: list[str] = []
    if core and core != entry:
        candidates.append(PurePosixPath(core).as_posix())

    parts = [part for part in core.split("/") if part and part != "."]
    parts = parts[-_MAX_TAIL_COMPONENTS:]
    candidates.extend("/".join(parts[start:]) for start in range(1, len(parts)))

    return list(dict.fromkeys(candidate for candidate in candidates if candidate != entry))


def _with_churn(files: tuple[render.MapFile, ...], ctx: RepoContext) -> tuple[render.MapFile, ...]:
    """Stamp each ranked file with its windowed commit activity.

    Data beside the rank, never folded into it: an unmeasured ranking change
    is what the 0.7.1 rollback withdrew, so the model gets the fact and does
    its own weighing. One bounded git call for the whole listing, after the
    ranking chose it, so an unranked repository file costs nothing.

    Every degradation leaves the files exactly as they came: no churn source
    on the context (a root outside git, a hand-built pinned context), or a
    lookup git could not answer. The header renders nothing for None, so a
    map cannot claim quiet history it never measured. A future-dated commit
    clamps to zero days rather than going negative.
    """
    if ctx.churn is None or not files:
        return files
    facts = ctx.churn.for_paths([entry.path for entry in files])
    if facts is None:
        return files
    now = int(time.time())
    decorated = []
    for entry in files:
        fact = facts.get(entry.path)
        if fact is None:
            decorated.append(entry)
            continue
        days = None
        if fact.last_commit_ts is not None:
            days = max((now - fact.last_commit_ts) // 86400, 0)
        decorated.append(replace(entry, commits_window=fact.commits, last_commit_days=days))
    return tuple(decorated)


@dataclass(frozen=True)
class BodyMapResult:
    """A body-granularity map: ranked file rows plus the top symbols' bodies.

    Composition, not a new view: the map half is the file-granularity shape
    (every entry stripped, counts kept), and the bodies half is a real
    :class:`ExpandResult` from the same machinery ``expand`` answers with --
    seats, water-filling, truncation markers and all. One call replaces the
    map-then-expand round trip, which is the cost 0.6.3 measured.
    """

    map: MapResult
    bodies: ExpandResult

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form: the map document with a ``bodies`` key."""
        return {**self.map.as_dict(), "bodies": self.bodies.as_dict()}


def build_body_map(
    ctx: RepoContext,
    request: MapRequest,
    maps: MapService,
    symbols: SymbolService,
) -> BodyMapResult:
    """Build the map, then expand its top symbols, in one answer.

    A symbol the focus named is seated first: ``MapResult.focus_order``
    carries those ids, and centrality orders every remaining seat. Without
    that, a caller who focused on a late-file symbol got the file's densest
    boilerplate expanded and still needed the second round trip this view
    exists to remove -- the packing's score is inbound-name centrality and
    never sees the focus. A focus that named only files seats nothing here,
    and selection stays the function-granularity map's own: the entries that
    won the signature packing, in the packing's score order, so when the
    seats are fewer than the ids the bodies are the highest-scored symbols
    overall, which is what the tool description advertises. The file rows
    re-sort by line for reading; ``MapResult.expand_order`` is what preserves
    the score order to here. The seat count is the budget divided by
    :data:`BODY_TOKENS_PER_SEAT`, capped at expand's own ceiling, and the
    bodies then share the same budget the map was asked for, water-filled.

    The map half is handed back at the file-granularity shape: signature
    rows beside whole bodies of the same symbols would say everything twice.
    Its ``rendered`` count is restated for the stripped rows, so the number
    describes the text this result actually renders.
    """
    base = maps.build(ctx, replace(request, granularity=GRANULARITY_FUNCTION))
    ids = list(dict.fromkeys((*base.focus_order, *base.expand_order)))
    seats = min(max(1, base.budget // BODY_TOKENS_PER_SEAT), EXPAND_MAX_SEATS)
    bodies = symbols.expand_symbols(
        ctx, ids[:seats], limit=max(seats, 1), budget=base.budget, seats=seats
    )
    stripped = tuple(replace(map_file, entries=()) for map_file in base.files)
    display = replace(
        base, files=stripped, rendered=maps._count_tokens(render.render_map(stripped))
    )
    return BodyMapResult(map=display, bodies=bodies)


def render_body_map(maps: MapService, result: BodyMapResult) -> str:
    """Render the file rows, then the bodies, one text.

    The unresolved rows ride in the body the way the MCP expand door carries
    them: a tool call has one channel. They should be empty -- every id came
    off the scan moments earlier -- so a row here is race evidence, not
    routine.
    """
    header = maps.render_text(result.map)
    body = "\n".join([render_expansion(result.bodies), *unresolved_lines(result.bodies)])
    return f"{header}\n{body}"


def _matches_stem(path: str, entry: str) -> bool:
    """Say whether ``entry`` is ``path`` with its extension stripped, or a suffix of it.

    ``envelope`` matches ``application/envelope.py``; so does
    ``application/envelope``. Only the path's final suffix comes off, and dots
    inside ``entry`` are treated as part of the name -- an entry spelling the
    file's real extension already had its chance as a literal path suffix.
    """
    bare = PurePosixPath(path).with_suffix("").as_posix()
    return bare == entry or bare.endswith(f"/{entry}")


def _score_symbols(
    paths: list[str],
    by_path: dict[str, refs.FileFacts],
    index: refs.RefIndex,
    rank: Mapping[str, float],
    stoplist: frozenset[str],
) -> list[_Candidate]:
    """Spread each file's rank across its symbols by inbound reference weight.

    A file's rank says how much the repository points at the file; the inbound
    reference count says which symbol inside it the repository is pointing at.
    Multiplying keeps both: a hot symbol in a cold file still loses to a warm
    symbol in a hot one, which is the ordering a funnel wants.

    The inbound count is a count of *bare-name* reference sites, so it is
    damped exactly the way the file-level edges are, through the same two
    functions. Undamped, a method spelled ``build`` or ``run`` outranks the
    symbol the repository really points at, and a name the repository declared
    in its own stoplist is still ranked by how often that spelling collides.
    :func:`agentless_mcp.core.graph.common_name_damping` says it is one home
    for the treatment of common names because two views ask the same question;
    this is the third view asking it.

    ``paths``, ``by_path`` and ``rank`` are keyed off the same scan, so a
    missing key is a desynchronisation and raises here rather than dropping
    the file from the map or ranking it alongside the genuinely cold ones. A
    name with no reference sites is a different case: it is a symbol nothing
    spells, not a mismatch.
    """
    candidates: list[_Candidate] = []
    for path in paths:
        facts = by_path[path]
        file_rank = rank[path]
        for symbol in facts.symbols:
            sites = index.sites.get(symbol.name, ())
            inbound = sum(1 for ref in sites if ref.path != path)
            spread = len({ref.path for ref in sites})
            weight = name_multiplier(symbol.name, stoplist) / common_name_damping(spread)
            candidates.append(
                _Candidate(score=file_rank * (1.0 + inbound * weight), path=path, symbol=symbol)
            )

    candidates.sort(key=lambda item: (-item.score, item.path, item.symbol.line_number))
    return candidates


def _group(shown: int, packing: "_Packing") -> tuple[render.MapFile, ...]:
    """Group the included symbols back into the ranked files that hold them.

    Every ranked file is listed, including the ones whose symbols all lost the
    budget and the ones that extracted no symbol at all: they appear with an
    empty body and their omitted count. A file that vanishes entirely because
    it placed no symbols is the bounded-view-mistaken-for-complete failure --
    the reader would have no way to know it was ever ranked.

    ``shown`` is how many of the eligible symbols to render, which is what
    the packing search bisects over -- the prefix, not a set, so a caller
    cannot ask for a selection the score ordering does not produce.

    :attr:`_Packing.paths` is the ranked file list rather than the paths the
    candidates happen to name, and that is the whole point of the field.
    Built from the candidates, this function dropped every ranked file with no
    extracted symbol -- systematically ``__init__.py`` and ``index.ts``, the
    files that name a package's public surface -- and neither the text nor the
    JSON said a file had gone. The list arrives already in rank order, so the
    order here is the ranking's, not one re-derived from a subset of it.
    """
    per_file: dict[str, list[_Candidate]] = {}
    for candidate in packing.eligible[:shown]:
        per_file.setdefault(candidate.path, []).append(candidate)

    totals: dict[str, int] = {}
    for candidate in packing.candidates:
        totals[candidate.path] = totals.get(candidate.path, 0) + 1

    files: list[render.MapFile] = []
    for path in packing.paths:
        chosen = sorted(per_file.get(path, []), key=lambda item: item.symbol.line_number)
        entries = tuple(
            render.MapEntry(
                line=candidate.symbol.line_number,
                signature=candidate.symbol.signature,
                stable_id=symbol_stable_id(candidate.symbol),
                depth=1 if candidate.symbol.parent_class else 0,
                rationales=rationale_nodes(candidate.symbol),
                name=id_qualname(candidate.symbol),
            )
            for candidate in chosen
        )
        files.append(
            render.MapFile(
                path=path,
                rank=packing.rank[path],
                entries=entries,
                total=totals.get(path, 0),
                id_prefix=language_prefix(chosen[0].symbol.language) if chosen else "",
                reached=path in packing.reached,
            )
        )
    return tuple(files)
