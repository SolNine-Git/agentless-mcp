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
  set's own size divided by six, clamped to 2k-8k tokens. Roughly 6x
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

from collections.abc import Collection, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import PurePosixPath
from typing import Any

from agentless_mcp.application import render
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import is_test_path, rationale_nodes
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
from agentless_mcp.core.symbols import ASTSymbol, qualname, symbol_stable_id
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util import bounds
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
GRANULARITIES = (GRANULARITY_FUNCTION, GRANULARITY_FILE)

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
        ranking = personalized_pagerank(graph, seeds or None)
        rank = ranking.rank
        chosen = rank_order(rank)[: max(0, max_files)]

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
                )
                for path in chosen
            )
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
        budget = (
            request.budget
            if request.budget is not None
            else self._auto_budget(candidates, chosen, rank)
        )
        included = self._pack(candidates, chosen, rank, budget)
        grouped = _group(candidates[:included], candidates, chosen, rank)

        return MapResult(
            files=grouped,
            budget=budget,
            included=included,
            candidates=len(candidates),
            ranked=len(rank),
            seeds=tuple(sorted(seeds)),
            skipped=scan.skipped,
            unresolved_seeds=seeding.unresolved,
            rendered=self._counter.count(render.render_map(grouped)),
            rank_converged=ranking.converged,
            test_companions=companions,
        )

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

    def _auto_budget(
        self,
        candidates: list[_Candidate],
        paths: Sequence[str],
        rank: Mapping[str, float],
    ) -> int:
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
            prefix = candidates[:size]
            rendered = self._counter.count(render.render_map(_group(prefix, prefix, paths, rank)))
            if rendered > AUTO_BUDGET_CEILING:
                return AUTO_BUDGET_MAX
            if size >= len(candidates):
                estimate = rendered // AUTO_BUDGET_DIVISOR
                return max(AUTO_BUDGET_MIN, min(AUTO_BUDGET_MAX, estimate))
            size *= AUTO_BUDGET_PROBE_GROWTH

    def _pack(
        self,
        candidates: list[_Candidate],
        paths: Sequence[str],
        rank: Mapping[str, float],
        budget: int,
    ) -> int:
        """Return the largest number of symbols whose render fits ``budget``."""
        if not candidates:
            return 0

        low, high = 0, len(candidates)
        while low < high:
            middle = (low + high + 1) // 2
            text = render.render_map(_group(candidates[:middle], candidates, paths, rank))
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

    for entry in focus:
        cleaned = entry.strip()
        if not cleaned:
            # A blank entry is the absence of an entry, not a seed that
            # failed to resolve -- the same thing an empty ``--focus`` string
            # already means -- so reporting it would name nothing back at the
            # caller. "Nothing is lost quietly" is about entries that spell
            # something the scan could not find.
            continue

        paths = focus_paths(cleaned, known, index)
        if not paths:
            unresolved.append(cleaned)
            continue

        share = 1.0 / len(paths)
        for path in paths:
            weights[path] = weights.get(path, 0.0) + share

    return Seeding(weights=weights, unresolved=tuple(dict.fromkeys(unresolved)))


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
    """
    normalized = PurePosixPath(entry).as_posix()
    if normalized in known:
        return [normalized]

    suffix_matches = sorted(path for path in known if path.endswith(f"/{normalized}"))
    if suffix_matches:
        return suffix_matches

    stem_matches = sorted(path for path in known if _matches_stem(path, normalized))
    if stem_matches:
        return stem_matches

    qualified = entry.rpartition("::")[2] or entry
    if "/" in qualified:
        return []

    name = qualified.rpartition(".")[2] or qualified
    defining = [
        definition for definition in index.definitions.get(name, ()) if definition.path in known
    ]
    owned = [definition for definition in defining if qualname(definition.symbol) == qualified]
    return sorted({definition.path for definition in (owned or defining)})


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


def _group(
    included: list[_Candidate],
    candidates: list[_Candidate],
    paths: Sequence[str],
    rank: Mapping[str, float],
) -> tuple[render.MapFile, ...]:
    """Group the included symbols back into the ranked files that hold them.

    Every ranked file is listed, including the ones whose symbols all lost the
    budget and the ones that extracted no symbol at all: they appear with an
    empty body and their omitted count. A file that vanishes entirely because
    it placed no symbols is the bounded-view-mistaken-for-complete failure --
    the reader would have no way to know it was ever ranked.

    ``paths`` is the ranked file list rather than the paths the candidates
    happen to name, and that is the whole point of the argument. Built from
    the candidates, this function dropped every ranked file with no extracted
    symbol -- systematically ``__init__.py`` and ``index.ts``, the files that
    name a package's public surface -- and neither the text nor the JSON said
    a file had gone. The list arrives already in rank order, so the order here
    is the ranking's, not one re-derived from a subset of it.
    """
    per_file: dict[str, list[_Candidate]] = {}
    for candidate in included:
        per_file.setdefault(candidate.path, []).append(candidate)

    totals: dict[str, int] = {}
    for candidate in candidates:
        totals[candidate.path] = totals.get(candidate.path, 0) + 1

    files: list[render.MapFile] = []
    for path in paths:
        chosen = sorted(per_file.get(path, []), key=lambda item: item.symbol.line_number)
        entries = tuple(
            render.MapEntry(
                line=candidate.symbol.line_number,
                signature=candidate.symbol.signature,
                stable_id=symbol_stable_id(candidate.symbol),
                depth=1 if candidate.symbol.parent_class else 0,
                rationales=rationale_nodes(candidate.symbol),
            )
            for candidate in chosen
        )
        files.append(
            render.MapFile(
                path=path,
                rank=rank[path],
                entries=entries,
                total=totals.get(path, 0),
            )
        )
    return tuple(files)
