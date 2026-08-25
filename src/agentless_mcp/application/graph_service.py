"""The structural views: explain, path, cycles, communities, diagram, HTML.

The graph is assembled **in memory, per call**, from the same repository scan
every other view is built from. Nothing about it is stored: there is no graph
database, no incremental update path and no watcher, because the answer to
"is this still true" has to be "it was computed from the tree you are looking
at" rather than "the index says so". The per-file sha256 gate behind
:class:`~agentless_mcp.core.cache.FileSource` is what makes that affordable --
the parses are reused, the resolution is not.

Five questions over two graphs. The first three are shapes of the same
symbol-level edge set:

``explain``
    One symbol, denormalized: where it is defined, what it references, what
    references it, and how its file sits in the import graph. Both fan sections
    are grouped by evidence tier so the strongest rows are read first.
``path``
    How two symbols are connected at all. Edges are walked as undirected --
    "how do these relate" is not a question about call direction -- and each
    hop is rendered with the direction the edge really runs. Unique and
    ambiguous name-only edges are excluded by default: a path built out of
    retrieval evidence reads like an architecture finding and is not one.
``cycles``
    Module-level import cycles, by strongly connected component. The one
    question in this module that is about files rather than symbols, because
    an import cycle is a property of modules.

The last two read the *file*-level graph the map ranks -- "which files mention
names these files define" -- because both are questions about modules and
neither needs a name bound to a declaration:

``communities``
    Which files belong together, by deterministic modularity, with a
    mechanical label per group. A rollup, not a ranking.
``diagram``
    The same graph as mermaid text, rank-bounded and optionally grouped by
    those communities, with declared imports drawn solid and name-reference
    edges dashed so the picture cannot contradict what ``cycles`` just said.
    Returned on demand and never written anywhere: a diagram is presentation
    for a human, and the facts stay in the flattened views an agent reads.
``html``
    A larger self-contained view of the same bounded graph with clickable
    nodes, community colours, and path search. The service only returns the
    document; the CLI decides between stdout and the XDG cache.

Every one of them is bounded and says what it left out.
"""

import math
from collections import Counter, defaultdict
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from types import MappingProxyType

from agentless_mcp.application import render
from agentless_mcp.application.map_service import focus_paths
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import (
    is_fixture_path,
    is_test_path,
    rationale_nodes,
    symbol_card,
)
from agentless_mcp.core import communities, graph, htmlgraph, mermaid, refs, resolve
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.symbols import SymbolKind, qualname, symbol_stable_id
from agentless_mcp.util import bounds
from agentless_mcp.util.errors import OperationFailed

DEFAULT_EXPLAIN_LIMIT = 20
DEFAULT_CYCLE_LIMIT = 20
DEFAULT_COMMUNITY_LIMIT = 20
DEFAULT_HEALTH_LIMIT = 20

# How many member paths one community lists before it elides. A community is
# usually a directory; past a dozen files the reader wants the label and the
# count, and `list_dir` is the view that enumerates.
DEFAULT_MEMBER_LIMIT = 12


@dataclass(frozen=True)
class PathOptions:
    """Bounds and evidence tiers accepted by a path query."""

    include_unique: bool = False
    include_ambiguous: bool = False
    max_visited: int = resolve.DEFAULT_MAX_VISITED


DEFAULT_PATH_OPTIONS = PathOptions()


@dataclass(frozen=True)
class DiagramRequest:
    """The knobs of one diagram call.

    A value object rather than five keyword arguments, for the reason
    :class:`agentless_mcp.core.mermaid.DiagramOptions` is one a layer down:
    they travel together from a CLI flag set or an MCP request, and a diagram
    is defined by them. It is also what lets ``max_edges`` reach the renderer
    at all -- a sixth parameter on the method is over the argument-count
    contract this package lints for.
    """

    focus: str | None = None
    max_nodes: int = mermaid.DEFAULT_DIAGRAM_NODES
    max_edges: int = mermaid.DEFAULT_DIAGRAM_EDGES
    group_by_communities: bool = False
    resolution: float | None = None


DEFAULT_DIAGRAM_REQUEST = DiagramRequest()

# Read as "the source <verb> the target", and its passive for a hop walked
# backwards. Rendering a reverse hop with the active verb would invert the
# meaning of every second line of a path.
_ACTIVE: dict[resolve.Relation, str] = {
    resolve.Relation.REFERENCES: "references",
    resolve.Relation.IMPORTS: "imports",
    resolve.Relation.INHERITS: "inherits",
}
_PASSIVE: dict[resolve.Relation, str] = {
    resolve.Relation.REFERENCES: "referenced by",
    resolve.Relation.IMPORTS: "imported by",
    resolve.Relation.INHERITS: "inherited by",
}

FORWARD_ARROW = "->"
REVERSE_ARROW = "<-"

# The evidence tiers a degree count trusts. A unique-name match says only
# that the repository spells that name once, and a name-only-ambiguous match
# says only that it spells it at all; counted as edges, either one reports a
# symbol as reached because a word appears somewhere. They are discounted
# rather than dropped -- every health row names the tiers behind the matches
# it did not count -- which is the fact the SQL this view is ported from
# cannot express, because its edges carry no tier.
_BINDING_TIERS = frozenset({resolve.Tier.SAME_FILE, resolve.Tier.IMPORTED})

# The one kind whose absence from the call graph is a finding. A constant, a
# type alias or an enum member is called by nothing and would fill an orphan
# listing with rows nobody can act on.
#
# Methods are excluded, and that is a departure from the SQL this view is
# ported from. A call through a selector -- ``obj.method()``, ``self.help()``
# -- is an attribute member, which :func:`resolve._reference_edges` refuses at
# every tier, so no method ever earns an inbound edge from its ordinary call
# site. Measured on this repository (2026-08-23): with methods included, all
# 184 orphan candidates and all 238 unused exports were methods and none was
# dead. A listing that is entirely one blind spot is a confident false
# positive, so the blind spot is named in the report instead.
_ORPHAN_KINDS = frozenset({SymbolKind.FUNCTION})

# The kinds a hub may be. Wider, because the symbol everything routes through
# is as often a class as a function, and methods belong here: the same missing
# selector edge only undercounts a degree, where above it manufactures a
# finding.
_HUB_KINDS = frozenset(
    {
        SymbolKind.FUNCTION,
        SymbolKind.METHOD,
        SymbolKind.CLASS,
        SymbolKind.DATACLASS,
        SymbolKind.PROTOCOL,
        SymbolKind.ENUM,
    }
)


@dataclass(frozen=True)
class _Resolved:
    """One call's scan, name index and resolved edge set."""

    index: refs.RefIndex
    graph: resolve.ResolvedGraph


@dataclass(frozen=True)
class _Ranked:
    """One call's file-level graph, the ranking over it, and its import pairs.

    ``imports`` is the subset of the graph's edges that a declared import
    accounts for. The file graph itself merges import weight and name-reference
    weight into one number per edge, so the kind has to be carried alongside
    it for any view that must not draw a reference as an import.
    """

    index: refs.RefIndex
    ranking: graph.PageRank
    graph: graph.RefGraph
    imports: frozenset[tuple[str, str]]

    @property
    def rank(self) -> Mapping[str, float]:
        """The score per file, without the convergence report around it."""
        return self.ranking.rank


@dataclass(frozen=True)
class _Focus:
    """A diagram's seed module, resolved, or the reason it is not one."""

    node: str
    message: str


class GraphService:
    """Answers structural questions from a graph rebuilt on every call."""

    def __init__(self, extractor: TreeSitterExtractor) -> None:
        self._extractor = extractor

    def explain(
        self,
        ctx: RepoContext,
        target: str,
        *,
        limit: int = DEFAULT_EXPLAIN_LIMIT,
    ) -> render.Explanation:
        """Build the card for one symbol: definition site, fan-out, fan-in, imports."""
        bounds.within(limit, 1, bounds.MAX_LIMIT, "limit")
        resolved = self._resolve(ctx)
        definitions = refs.definitions_for(resolved.index, target)
        if not definitions:
            return _missing(target)

        ordered = rank_candidates(definitions, target)
        chosen = ordered[0]
        node = symbol_stable_id(chosen.symbol)

        return render.Explanation(
            target=target,
            card=symbol_card(chosen.symbol),
            message="",
            alternatives=tuple(symbol_stable_id(entry.symbol) for entry in ordered[1:]),
            rationales=rationale_nodes(chosen.symbol),
            fan_out=_tier_groups(resolved.graph.outgoing().get(node, ()), limit, outgoing=True),
            fan_in=_tier_groups(resolved.graph.incoming().get(node, ()), limit, outgoing=False),
            imports_out=_imports(resolved.graph, chosen.path, limit, declared=True),
            imports_in=_imports(resolved.graph, chosen.path, limit, declared=False),
        )

    def path(
        self,
        ctx: RepoContext,
        source: str,
        target: str,
        options: PathOptions = DEFAULT_PATH_OPTIONS,
    ) -> render.PathTrace:
        """Find the fewest-hop connection between two symbols or files."""
        bounds.at_least(options.max_visited, 1, "max_visited")
        resolved = self._resolve(ctx)
        start = _endpoint(resolved, source)
        finish = _endpoint(resolved, target)
        if start.message or finish.message:
            return _unresolved_path(
                source,
                target,
                start,
                finish,
                options,
            )

        found = resolve.shortest_path(
            resolved.graph,
            start.node,
            finish.node,
            edge_policy=resolve.PathEdgePolicy(
                include_unique=options.include_unique,
                include_ambiguous=options.include_ambiguous,
            ),
            max_visited=options.max_visited,
        )
        return _trace(
            start,
            finish,
            found,
            include_unique=options.include_unique,
            include_ambiguous=options.include_ambiguous,
        )

    def cycles(self, ctx: RepoContext, *, limit: int = DEFAULT_CYCLE_LIMIT) -> render.CycleReport:
        """Report every module-level import cycle, in a deterministic order."""
        bounds.within(limit, 1, bounds.MAX_LIMIT, "limit")
        resolved = self._resolve(ctx)
        cycles = resolve.import_cycles(resolved.graph)
        return render.CycleReport(
            cycles=tuple(render.CycleRow(files=cycle.files) for cycle in cycles[:limit]),
            total=len(cycles),
            limit=limit,
            unresolved_imports=resolved.graph.unresolved_imports,
        )

    def health(self, ctx: RepoContext, *, limit: int = DEFAULT_HEALTH_LIMIT) -> render.HealthReport:
        """Report orphans, unused exports and hubs over the resolved graph.

        Three findings in one answer rather than three calls, because they are
        three readings of one degree count and computing that count is what
        the call costs. ``limit`` bounds each section separately, and each
        section carries the size of the finding it was cut from.
        """
        bounds.within(limit, 1, bounds.MAX_LIMIT, "limit")
        resolved = self._resolve(ctx)
        return health_report(resolved.graph, limit=limit)

    def communities(
        self,
        ctx: RepoContext,
        *,
        resolution: float | None = None,
        limit: int = DEFAULT_COMMUNITY_LIMIT,
        members: int = DEFAULT_MEMBER_LIMIT,
    ) -> render.CommunityReport:
        """Partition the repository's files into communities, largest first."""
        bounds.within(limit, 1, bounds.MAX_LIMIT, "limit")
        bounds.at_least(members, 1, "members")
        ranked = self._ranked(ctx)
        partition = communities.detect_communities(ranked.graph, resolution=_resolution(resolution))
        return render.CommunityReport(
            communities=tuple(
                render.CommunityRow(
                    label=community.label,
                    total=community.size,
                    members=community.members[:members],
                    internal_weight=community.internal_weight,
                    total_weight=community.total_weight,
                    limit=members,
                )
                for community in partition.communities[:limit]
            ),
            total=len(partition.communities),
            limit=limit,
            modularity=partition.modularity,
            standard_modularity=partition.standard_modularity,
            resolution=partition.resolution,
            files=len(ranked.graph.nodes),
        )

    def diagram(
        self,
        ctx: RepoContext,
        request: DiagramRequest = DEFAULT_DIAGRAM_REQUEST,
    ) -> render.DiagramView:
        """Render the module-level graph as mermaid text, edge kinds distinguished.

        On demand and nowhere else: the text is returned, never written, and
        the repository under analysis is read exactly as every other view
        reads it. Declared imports draw solid, name-reference edges dashed and
        only while the diagram stays legible -- drawn identically, a pair of
        opposite reference edges reads as an import cycle the ``cycles`` view
        would deny.

        """
        bounds.within(request.max_nodes, 1, bounds.MAX_DIAGRAM_NODES, "max_nodes")
        bounds.within(request.max_edges, 0, bounds.MAX_DIAGRAM_EDGES, "max_edges")
        ranked = self._ranked(ctx)
        seed = _diagram_focus(request.focus, ranked)
        if seed.message:
            return _empty_diagram(
                request.focus or "", seed.message, grouped=request.group_by_communities
            )

        # Parsed outside the branch below on purpose: a caller who mistypes
        # the knob hears about it whether or not they asked for grouping.
        setting = _resolution(request.resolution)
        partition = (
            communities.detect_communities(ranked.graph, resolution=setting)
            if request.group_by_communities
            else None
        )
        options = mermaid.DiagramOptions(
            max_nodes=request.max_nodes,
            max_edges=request.max_edges,
            focus=seed.node or None,
        )
        # Every count below comes off the one render. Read back through
        # `selected_nodes` instead, the counts cost two more neighbourhood
        # walks and are produced by a second code path from the numbers
        # already inside the picture.
        drawn = mermaid.render_flowchart(
            ranked.graph,
            ranked.rank,
            partition=partition,
            options=options,
            imports=ranked.imports,
        )
        return render.DiagramView(
            text=drawn.text,
            nodes=drawn.nodes,
            elided=drawn.elided_nodes,
            grouped=partition is not None,
            focus=options.focus or "",
            message="",
            edges_over_bound=drawn.edges_over_bound,
            rank_converged=ranked.ranking.converged,
        )

    def html(
        self,
        ctx: RepoContext,
        *,
        max_nodes: int = htmlgraph.DEFAULT_HTML_NODES,
        max_edges: int = htmlgraph.DEFAULT_HTML_EDGES,
        resolution: float | None = None,
    ) -> htmlgraph.HtmlExport:
        """Render an interactive module graph without persisting graph state."""
        bounds.within(max_nodes, 1, htmlgraph.MAX_HTML_NODES, "max_nodes")
        bounds.within(max_edges, 0, htmlgraph.MAX_HTML_EDGES, "max_edges")
        ranked = self._ranked(ctx)
        partition = communities.detect_communities(
            ranked.graph,
            resolution=_resolution(resolution),
        )
        return htmlgraph.render_html(
            ranked.graph,
            ranked.rank,
            partition,
            imports=ranked.imports,
            options=htmlgraph.HtmlOptions(max_nodes=max_nodes, max_edges=max_edges),
        )

    def _resolve(self, ctx: RepoContext) -> _Resolved:
        """Scan the repository and resolve every reference it holds."""
        scan = refs.scan_repo(ctx.root, self._extractor, source=ctx.symbols)
        index = refs.build_ref_index(scan)
        _, resolved = resolve.resolve_repo(scan, index)
        return _Resolved(index=index, graph=resolved)

    def _ranked(self, ctx: RepoContext) -> _Ranked:
        """Build the file-level graph and its ranking, the map's own two steps.

        Communities and diagrams are both shapes of the *file* graph, not of
        the symbol graph the other three views read, so they take the cheaper
        half of the pipeline and skip reference resolution entirely.
        """
        scan = refs.scan_repo(ctx.root, self._extractor, source=ctx.symbols)
        index = refs.build_ref_index(scan)
        built = graph.build_graph(
            scan,
            index,
            stoplist=ctx.config.stoplist,
            relation_weights=bool(ctx.config.relation_weights),
        )
        return _Ranked(
            index=index,
            graph=built,
            ranking=graph.personalized_pagerank(
                built,
                pure_sources={path for path in built.nodes if is_test_path(path)},
            ),
            imports=_import_pairs(scan),
        )


@dataclass(frozen=True)
class _Degree:
    """One symbol's counted edges, and the weak matches that were not counted."""

    counted_in: int = 0
    counted_out: int = 0
    discounted_in: Mapping[resolve.Tier, int] = MappingProxyType({})
    discounted_out: Mapping[resolve.Tier, int] = MappingProxyType({})

    @property
    def counted(self) -> int:
        """The counted edges at both ends."""
        return self.counted_in + self.counted_out


_NO_DEGREE = _Degree()


def health_report(graph: resolve.ResolvedGraph, *, limit: int) -> render.HealthReport:
    """Compute the three structural-health findings over one resolved graph.

    Pure: one graph in, one report out, no repository and no clock. Test and
    fixture paths are excluded before anything is counted, because a fixture
    that exists to be parsed and a test helper nothing calls are permanent
    orphans and would be the whole listing on most repositories.
    """
    degrees = _degrees(graph)
    considered = [
        (stable_id, definition)
        for stable_id, definition in sorted(graph.definitions.items())
        if not is_test_path(definition.path) and not is_fixture_path(definition.path)
    ]

    orphans = [
        (stable_id, definition, degree)
        for stable_id, definition, degree in _of_kinds(considered, degrees, _ORPHAN_KINDS)
        if not degree.counted
    ]
    unused = [
        (stable_id, definition, degree)
        for stable_id, definition, degree in _of_kinds(considered, degrees, _ORPHAN_KINDS)
        if definition.symbol.is_public and not degree.counted_in
    ]
    hubs = sorted(
        (entry for entry in _of_kinds(considered, degrees, _HUB_KINDS) if entry[2].counted),
        key=lambda entry: (-entry[2].counted, entry[1].path, entry[1].symbol.line_number),
    )

    return render.HealthReport(
        orphans=_health_section(orphans, limit=limit, inbound_only=False),
        unused_exports=_health_section(unused, limit=limit, inbound_only=True),
        hubs=_health_section(hubs, limit=limit, inbound_only=False),
        symbols=len(considered),
        excluded=len(graph.definitions) - len(considered),
    )


def _of_kinds(
    considered: Sequence[tuple[str, refs.Definition]],
    degrees: Mapping[str, _Degree],
    kinds: frozenset[SymbolKind],
) -> list[tuple[str, refs.Definition, _Degree]]:
    """Keep the definitions of the named kinds, each beside its degree.

    Ordered by ``(path, line)`` -- the order the ported SQL sorts by -- with
    the stable id breaking a tie, because two symbols can start on one line
    and a listing whose order depends on dictionary insertion is not a
    listing a golden can pin.
    """
    return sorted(
        (
            (stable_id, definition, degrees.get(stable_id, _NO_DEGREE))
            for stable_id, definition in considered
            if definition.symbol.kind in kinds
        ),
        key=lambda entry: (entry[1].path, entry[1].symbol.line_number, entry[0]),
    )


def _degrees(graph: resolve.ResolvedGraph) -> dict[str, _Degree]:
    """Count each node's edges in one pass, keeping the tiers apart.

    Both ends of every edge are counted here rather than through
    :meth:`ResolvedGraph.incoming` and :meth:`ResolvedGraph.outgoing`, which
    build two grouped copies of the whole edge list to answer a question that
    is two integers per node.
    """
    counted_in: Counter[str] = Counter()
    counted_out: Counter[str] = Counter()
    weak_in: defaultdict[str, Counter[resolve.Tier]] = defaultdict(Counter)
    weak_out: defaultdict[str, Counter[resolve.Tier]] = defaultdict(Counter)

    for edge in graph.edges:
        if edge.tier in _BINDING_TIERS:
            counted_out[edge.source.node] += 1
            counted_in[edge.target.node] += 1
        else:
            weak_out[edge.source.node][edge.tier] += 1
            weak_in[edge.target.node][edge.tier] += 1

    nodes = set(counted_in) | set(counted_out) | set(weak_in) | set(weak_out)
    return {
        node: _Degree(
            counted_in=counted_in[node],
            counted_out=counted_out[node],
            discounted_in=MappingProxyType(dict(weak_in[node])),
            discounted_out=MappingProxyType(dict(weak_out[node])),
        )
        for node in nodes
    }


def _health_section(
    entries: Sequence[tuple[str, refs.Definition, _Degree]],
    *,
    limit: int,
    inbound_only: bool,
) -> render.HealthSection:
    """Cap one finding and keep the count it was cut from.

    ``inbound_only`` selects which discounted matches a row names. An unused
    export is a claim about what reaches the symbol, so naming the weak
    matches leaving it would answer a question the row does not ask.
    """
    rows = tuple(
        _health_row(stable_id, definition, degree, inbound_only=inbound_only)
        for stable_id, definition, degree in entries[:limit]
    )
    return render.HealthSection(rows=rows, total=len(entries), limit=limit)


def _health_row(
    stable_id: str,
    definition: refs.Definition,
    degree: _Degree,
    *,
    inbound_only: bool,
) -> render.HealthSymbol:
    """Render one symbol's degree into a row, tiers named."""
    discounted: Counter[resolve.Tier] = Counter(degree.discounted_in)
    if not inbound_only:
        discounted.update(degree.discounted_out)
    return render.HealthSymbol(
        stable_id=stable_id,
        path=definition.path,
        line=definition.symbol.line_number,
        label=qualname(definition.symbol),
        kind=definition.symbol.kind.value,
        in_degree=degree.counted_in,
        out_degree=degree.counted_out,
        discounted=tuple(
            render.DiscountedTier(tier=tier.label, count=discounted[tier])
            for tier in resolve.TIER_ORDER
            if discounted[tier]
        ),
    )


def _import_pairs(scan: refs.RepoScan) -> frozenset[tuple[str, str]]:
    """Return the ``(importer, imported)`` file pairs a declared import connects.

    Read off :meth:`agentless_mcp.core.resolve.ImportScope.resolved_edges`,
    the one owner of "which resolved statement becomes an import edge", so the
    diagram's solid edges cannot disagree with explain, path and cycles -- in
    particular the submodule form ``from pkg import mod``, which only the
    scopes resolve to the module file. The built graph cannot answer this
    because it keeps only the merged edge weight and not the kind, and
    deriving the kind from the weight would be a proxy guard: enough name
    references sum past the import weight.
    """
    pairs: set[tuple[str, str]] = set()
    for path, scope in resolve.build_file_scopes(scan.files).items():
        for _module, _line, target in scope.resolved_edges(path):
            pairs.add((path, target))
    return frozenset(pairs)


@dataclass(frozen=True)
class _Located:
    """One path endpoint the caller named, resolved to a node or to a refusal."""

    node: str
    label: str
    message: str


def rank_candidates(
    definitions: Sequence[refs.Definition],
    target: str,
) -> tuple[refs.Definition, ...]:
    """Order the definitions a lookup target matched, best match first.

    :func:`agentless_mcp.core.refs.definitions_for` matches on the *last*
    segment of a qualified name, because that is what makes ``Invoice.total``
    findable when the caller does not know which class it lives on. The cost is
    that ``Resolver.resolve`` also matches ``ToolHandlers.resolve`` and a
    module-level ``resolve``, and a plain path-order sort would then hand back
    whichever file sorts first -- an exactly-named symbol losing to a
    coincidence.

    So the order is evidence-first: a definition whose qualified name *is* the
    text the caller typed outranks one that merely ends with it, and only
    within a band does ``(path, line)`` decide. Callers that refuse ambiguity
    look at the first band alone, which is why "several things end in
    ``.resolve``" stops being an ambiguity the moment one of them is spelled
    out in full.
    """
    return tuple(
        sorted(
            definitions,
            key=lambda entry: (
                0 if qualname(entry.symbol) == target else 1,
                entry.path,
                entry.symbol.line_number,
            ),
        )
    )


def _endpoint(resolved: _Resolved, text: str) -> _Located:
    """Resolve one endpoint argument to a graph node, or say why it is not one.

    A stable id and a repository-relative path are exact. A bare name is
    accepted only when the best-matching band holds exactly one symbol:
    several definitions of equal standing is a question the caller has to
    answer, and picking one would put a guess at the end of a path that reads
    as evidence.
    """
    if text in resolved.graph.definitions:
        definition = resolved.graph.definitions[text]
        return _Located(node=text, label=qualname(definition.symbol), message="")
    if text in resolved.graph.files:
        return _Located(node=text, label=text, message="")

    candidates = refs.definitions_for(resolved.index, text)
    if not candidates:
        return _Located(node="", label=text, message=f"no symbol or file matches {text}")

    ranked = rank_candidates(candidates, text)
    exact = [entry for entry in ranked if qualname(entry.symbol) == text]
    best = tuple(exact) if exact else ranked
    if len(best) > 1:
        listed = ", ".join(symbol_stable_id(entry.symbol) for entry in best)
        return _Located(node="", label=text, message=f"{text} is ambiguous: {listed}")
    return _Located(
        node=symbol_stable_id(best[0].symbol),
        label=qualname(best[0].symbol),
        message="",
    )


def _missing(target: str) -> render.Explanation:
    """Build the explanation for a target nothing in the repository defines."""
    return render.Explanation(
        target=target,
        card=None,
        message=f"no symbol matches {target}",
        alternatives=(),
        rationales=(),
        fan_out=(),
        fan_in=(),
        imports_out=render.ImportListing(),
        imports_in=render.ImportListing(),
    )


def _tier_groups(
    edges: tuple[resolve.SymbolEdge, ...],
    limit: int,
    *,
    outgoing: bool,
) -> tuple[render.TierGroup, ...]:
    """Group one side of a symbol's edges by tier, strongest tier first."""
    buckets: dict[resolve.Tier, list[render.EdgeRow]] = {}
    for edge in edges:
        endpoint = edge.target if outgoing else edge.source
        verb = _ACTIVE[edge.relation] if outgoing else _PASSIVE[edge.relation]
        buckets.setdefault(edge.tier, []).append(
            render.EdgeRow(
                node=endpoint.node,
                label=endpoint.label,
                path=endpoint.path,
                line=endpoint.line,
                relation=verb,
            )
        )

    return tuple(
        render.TierGroup(
            tier=tier.value,
            tier_label=tier.label,
            rows=tuple(buckets[tier][:limit]),
            total=len(buckets[tier]),
        )
        for tier in resolve.TIER_ORDER
        if tier in buckets
    )


def _imports(
    graph: resolve.ResolvedGraph,
    path: str,
    limit: int,
    *,
    declared: bool,
) -> render.ImportListing:
    """Return the import edges leaving or arriving at one file, bounded and counted.

    The count is taken before the slice. Twenty rows out of thirty importers
    is the section this module's docstring promises does not exist, and it was
    the one bounded view here whose total was never computed at all -- so
    neither the text nor the JSON had anything to be honest with.
    """
    rows = [
        render.ImportRow(
            path=edge.source.path,
            line=edge.source.line,
            module=edge.name,
            other=edge.target.path,
        )
        for edge in graph.import_edges()
        if (edge.source.node == path if declared else edge.target.node == path)
    ]
    return render.ImportListing(rows=tuple(rows[:limit]), total=len(rows), limit=limit)


def _resolution(resolution: float | None) -> float:
    """Parse the modularity resolution knob, or refuse it.

    ``float()`` is a coercion, not a parse: it accepts NaN and both
    infinities, and a NaN makes every gain comparison in the clustering false,
    so the answer is one singleton "community" per file and a modularity of
    NaN -- which `json.dumps` then emits as the bare token ``NaN``, invalid
    JSON for any strict parser on the other side. A negative resolution
    reports a modularity of 6.0 against a documented 0-to-1 scale. Both are
    refused here, at the one place both public methods that take the knob
    cross.

    The floor stays local because it is exclusive and
    :func:`agentless_mcp.util.bounds.within` is inclusive at both ends: zero
    is not a resolution, where the ceiling is a value the caller may ask for.
    The ceiling itself is the shared one, so the command line refuses
    ``--resolution 1e9`` exactly as the MCP schema already did.
    """
    if resolution is None:
        return communities.DEFAULT_RESOLUTION
    setting = float(resolution)
    if not math.isfinite(setting) or setting <= 0.0:
        message = f"resolution must be a finite number greater than 0, got {resolution}"
        raise OperationFailed(message)
    bounds.within(setting, 0.0, bounds.MAX_RESOLUTION, "resolution")
    return setting


def _trace(
    start: _Located,
    finish: _Located,
    found: resolve.PathResult,
    *,
    include_unique: bool,
    include_ambiguous: bool,
) -> render.PathTrace:
    """Turn a core path result into the view the adapters render."""
    hops = tuple(
        render.PathHop(
            verb=(_ACTIVE if hop.forward else _PASSIVE)[hop.edge.relation],
            tier=hop.edge.tier.value,
            tier_label=hop.edge.tier.label,
            arrow=FORWARD_ARROW if hop.forward else REVERSE_ARROW,
            node=hop.arrival.node,
            label=hop.arrival.label,
            path=hop.arrival.path,
            line=hop.arrival.line,
        )
        for hop in found.hops
    )
    return render.PathTrace(
        source=start.node,
        target=finish.node,
        source_label=start.label,
        target_label=finish.label,
        hops=hops,
        found=found.found,
        message=_path_message(
            found,
            include_unique=include_unique,
            include_ambiguous=include_ambiguous,
        ),
        visited=found.visited,
        exhausted=found.exhausted,
        include_unique=include_unique,
        include_ambiguous=include_ambiguous,
        endpoints_resolved=True,
    )


def _path_message(
    found: resolve.PathResult,
    *,
    include_unique: bool,
    include_ambiguous: bool,
) -> str:
    """Say what happened when a path search did not answer with hops.

    "Nothing connects these" and "I stopped looking" are different facts and
    are never merged: the second one names the bound it hit.

    The exhausted message used to advise raising the bound without saying
    where the bound lives. ``max_visited`` is a CLI flag and the MCP ``path``
    operation does not publish it, so an agent following the advice reissued
    the identical call. Naming the flag is what makes the sentence actionable
    for the caller who has it and honest for the caller who does not.
    """
    if found.found:
        return ""
    if found.exhausted:
        return (
            f"no path from {found.source} to {found.target} within the search bound "
            f"({found.visited} nodes visited); pick a nearer pair of endpoints, or "
            "raise --max-visited from the CLI"
        )
    excluded: list[str] = []
    if not include_unique:
        excluded.append("unique")
    if not include_ambiguous:
        excluded.append("name-only-ambiguous")
    hint = f"; {' and '.join(excluded)} edges were excluded" if excluded else ""
    return f"no path from {found.source} to {found.target} over resolved edges{hint}"


def _unresolved_path(
    source: str,
    target: str,
    start: _Located,
    finish: _Located,
    options: PathOptions,
) -> render.PathTrace:
    """Build the answer for a path whose endpoints could not be resolved."""
    reasons = [entry.message for entry in (start, finish) if entry.message]
    return render.PathTrace(
        source=source,
        target=target,
        source_label=start.label,
        target_label=finish.label,
        hops=(),
        found=False,
        message="; ".join(reasons),
        visited=0,
        exhausted=False,
        include_unique=options.include_unique,
        include_ambiguous=options.include_ambiguous,
        endpoints_resolved=False,
    )


def _diagram_focus(focus: str | None, ranked: _Ranked) -> _Focus:
    """Resolve a diagram's focus argument to one module, or say why not.

    The same resolution the map's ``--focus`` uses -- a path, a path suffix, a
    module stem or a symbol name -- narrowed to a single module, because a
    diagram has one centre. A focus naming several modules is answered with
    the list rather than with whichever one sorts first.
    """
    if focus is None or not focus.strip():
        return _Focus(node="", message="")

    entry = focus.strip()
    matches = focus_paths(entry, set(ranked.graph.nodes), ranked.index)
    if not matches:
        return _Focus(node="", message=f"no module matches {entry}")
    if len(matches) > 1:
        listed = ", ".join(matches)
        return _Focus(node="", message=f"{entry} matches several modules: {listed}")
    return _Focus(node=matches[0], message="")


def _empty_diagram(focus: str, message: str, *, grouped: bool) -> render.DiagramView:
    """Build the answer for a diagram whose focus resolved to nothing."""
    return render.DiagramView(
        text="",
        nodes=0,
        elided=0,
        grouped=grouped,
        focus=focus,
        message=message,
    )
