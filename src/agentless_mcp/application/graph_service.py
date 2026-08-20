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
from collections.abc import Sequence
from dataclasses import dataclass, replace

from agentless_mcp.application import render
from agentless_mcp.application.map_service import focus_paths
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import rationale_nodes, symbol_card
from agentless_mcp.core import communities, graph, htmlgraph, mermaid, refs, resolve
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.symbols import qualname, symbol_stable_id
from agentless_mcp.util.errors import AtlasError

DEFAULT_EXPLAIN_LIMIT = 20
DEFAULT_CYCLE_LIMIT = 20
DEFAULT_COMMUNITY_LIMIT = 20

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
    graph: graph.RefGraph
    rank: dict[str, float]
    imports: frozenset[tuple[str, str]]


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
        resolved = self._resolve(ctx)
        cycles = resolve.import_cycles(resolved.graph)
        return render.CycleReport(
            cycles=tuple(render.CycleRow(files=cycle.files) for cycle in cycles[:limit]),
            total=len(cycles),
            limit=limit,
        )

    def communities(
        self,
        ctx: RepoContext,
        *,
        resolution: float | None = None,
        limit: int = DEFAULT_COMMUNITY_LIMIT,
        members: int = DEFAULT_MEMBER_LIMIT,
    ) -> render.CommunityReport:
        """Partition the repository's files into communities, largest first."""
        ranked = self._ranked(ctx)
        partition = communities.detect_communities(ranked.graph, resolution=_resolution(resolution))
        return render.CommunityReport(
            communities=tuple(
                render.CommunityRow(
                    label=community.label,
                    size=community.size,
                    members=community.members[:members],
                    omitted=max(0, community.size - members),
                    internal_weight=community.internal_weight,
                    total_weight=community.total_weight,
                )
                for community in partition.communities[:limit]
            ),
            total=len(partition.communities),
            limit=limit,
            modularity=partition.modularity,
            resolution=partition.resolution,
            files=len(ranked.graph.nodes),
        )

    def diagram(
        self,
        ctx: RepoContext,
        *,
        focus: str | None = None,
        max_nodes: int = mermaid.DEFAULT_MAX_NODES,
        group_by_communities: bool = False,
        resolution: float | None = None,
    ) -> render.DiagramView:
        """Render the module-level graph as mermaid text, edge kinds distinguished.

        On demand and nowhere else: the text is returned, never written, and
        the repository under analysis is read exactly as every other view
        reads it. Declared imports draw solid, name-reference edges dashed and
        only while the diagram stays legible -- drawn identically, a pair of
        opposite reference edges reads as an import cycle the ``cycles`` view
        would deny.
        """
        ranked = self._ranked(ctx)
        seed = _diagram_focus(focus, ranked)
        if seed.message:
            return _empty_diagram(focus or "", seed.message, grouped=group_by_communities)

        setting = _resolution(resolution)
        partition = (
            communities.detect_communities(ranked.graph, resolution=setting)
            if group_by_communities
            else None
        )
        options = mermaid.DiagramOptions(max_nodes=max_nodes, focus=seed.node or None)
        drawn = mermaid.selected_nodes(ranked.graph, ranked.rank, options)
        text = mermaid.render_flowchart(
            ranked.graph,
            ranked.rank,
            partition=partition,
            options=options,
            imports=ranked.imports,
        )
        return render.DiagramView(
            text=text,
            nodes=len(drawn),
            elided=_elided(ranked, options, drawn),
            grouped=partition is not None,
            focus=options.focus or "",
            message="",
        )

    def html(
        self,
        ctx: RepoContext,
        *,
        max_nodes: int = htmlgraph.DEFAULT_MAX_NODES,
        max_edges: int = htmlgraph.DEFAULT_MAX_EDGES,
        resolution: float | None = None,
    ) -> htmlgraph.HtmlExport:
        """Render an interactive module graph without persisting graph state."""
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
        built = graph.build_graph(scan, index, stoplist=ctx.config.stoplist)
        return _Ranked(
            index=index,
            graph=built,
            rank=graph.personalized_pagerank(built),
            imports=_import_pairs(scan),
        )


def _import_pairs(scan: refs.RepoScan) -> frozenset[tuple[str, str]]:
    """Return the ``(importer, imported)`` file pairs a declared import connects.

    Read off :func:`agentless_mcp.core.resolve.build_file_scopes`, the one
    owner of import resolution, so the diagram's solid edges cannot disagree
    with explain, path and cycles -- in particular the submodule form
    ``from pkg import mod``, which only the scopes resolve to the module file.
    The built graph cannot answer this because it keeps only the merged edge
    weight and not the kind, and deriving the kind from the weight would be a
    proxy guard: enough name references sum past the import weight.
    """
    pairs: set[tuple[str, str]] = set()
    for path, scope in resolve.build_file_scopes(scan.files).items():
        for _module, _line, target in scope.statements:
            if target and target != path:
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
    """
    if resolution is None:
        return communities.DEFAULT_RESOLUTION
    setting = float(resolution)
    if not math.isfinite(setting) or setting <= 0.0:
        message = f"resolution must be a finite number greater than 0, got {resolution}"
        raise AtlasError(message)
    return setting


def _elided(ranked: _Ranked, options: mermaid.DiagramOptions, drawn: Sequence[str]) -> int:
    """Count what this diagram left out, against what it was drawing from.

    A focus restricts the candidate set before the rank bound is applied, and
    the elision node the picture carries counts against that restricted set.
    Counting against the whole repository instead made one response say "12
    elided" over a picture that had dropped nothing, and sent readers to raise
    `max_nodes` for modules no bound had removed. The candidate set is read
    back through the same public entry point the render uses, rather than
    re-deriving the neighbourhood walk here, so the two numbers cannot drift.
    """
    unbounded = replace(options, max_nodes=max(1, len(ranked.graph.nodes)))
    candidates = mermaid.selected_nodes(ranked.graph, ranked.rank, unbounded)
    return max(0, len(candidates) - len(drawn))


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
    """
    if found.found:
        return ""
    if found.exhausted:
        return (
            f"no path from {found.source} to {found.target} within the search bound "
            f"({found.visited} nodes visited); raise the bound or pick a nearer endpoint"
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
