"""The three views over the resolved symbol graph: explain, path, cycles.

The graph is assembled **in memory, per call**, from the same repository scan
every other view is built from. Nothing about it is stored: there is no graph
database, no incremental update path and no watcher, because the answer to
"is this still true" has to be "it was computed from the tree you are looking
at" rather than "the index says so". The per-file sha256 gate behind
:class:`~agentless_mcp.core.cache.FileSource` is what makes that affordable --
the parses are reused, the resolution is not.

Three questions, and each is a different shape of the same edge set:

``explain``
    One symbol, denormalized: where it is defined, what it references, what
    references it, and how its file sits in the import graph. Both fan sections
    are grouped by evidence tier so the strongest rows are read first.
``path``
    How two symbols are connected at all. Edges are walked as undirected --
    "how do these relate" is not a question about call direction -- and each
    hop is rendered with the direction the edge really runs. Ambiguous edges
    are excluded by default: a path built out of guessed bindings reads like a
    finding and is not one.
``cycles``
    Module-level import cycles, by strongly connected component. The one
    question in this module that is about files rather than symbols, because
    an import cycle is a property of modules.

Every one of them is bounded and says what it left out.
"""

from dataclasses import dataclass

from agentless_mcp.application import render
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import symbol_card
from agentless_mcp.core import refs, resolve
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.symbols import qualname, symbol_stable_id

DEFAULT_EXPLAIN_LIMIT = 20
DEFAULT_CYCLE_LIMIT = 20

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

        ordered = sorted(definitions, key=lambda entry: (entry.path, entry.symbol.line_number))
        chosen = ordered[0]
        node = symbol_stable_id(chosen.symbol)

        return render.Explanation(
            target=target,
            card=symbol_card(chosen.symbol),
            message="",
            alternatives=tuple(symbol_stable_id(entry.symbol) for entry in ordered[1:]),
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
        *,
        include_ambiguous: bool = False,
        max_visited: int = resolve.DEFAULT_MAX_VISITED,
    ) -> render.PathTrace:
        """Find the fewest-hop connection between two symbols or files."""
        resolved = self._resolve(ctx)
        start = _endpoint(resolved, source)
        finish = _endpoint(resolved, target)
        if start.message or finish.message:
            return _unresolved_path(
                source, target, start, finish, include_ambiguous=include_ambiguous
            )

        found = resolve.shortest_path(
            resolved.graph,
            start.node,
            finish.node,
            include_ambiguous=include_ambiguous,
            max_visited=max_visited,
        )
        return _trace(start, finish, found, include_ambiguous=include_ambiguous)

    def cycles(self, ctx: RepoContext, *, limit: int = DEFAULT_CYCLE_LIMIT) -> render.CycleReport:
        """Report every module-level import cycle, in a deterministic order."""
        resolved = self._resolve(ctx)
        cycles = resolve.import_cycles(resolved.graph)
        return render.CycleReport(
            cycles=tuple(render.CycleRow(files=cycle.files) for cycle in cycles[:limit]),
            total=len(cycles),
            limit=limit,
        )

    def _resolve(self, ctx: RepoContext) -> _Resolved:
        """Scan the repository and resolve every reference it holds."""
        scan = refs.scan_repo(ctx.root, self._extractor, source=ctx.symbols)
        index = refs.build_ref_index(scan)
        _, graph = resolve.resolve_repo(scan, index)
        return _Resolved(index=index, graph=graph)


@dataclass(frozen=True)
class _Located:
    """One path endpoint the caller named, resolved to a node or to a refusal."""

    node: str
    label: str
    message: str


def _endpoint(resolved: _Resolved, text: str) -> _Located:
    """Resolve one endpoint argument to a graph node, or say why it is not one.

    A stable id and a repository-relative path are exact. A bare name is
    accepted only when it names exactly one symbol: several definitions is a
    question the caller has to answer, and picking one would put a guess at the
    end of a path that reads as evidence.
    """
    if text in resolved.graph.definitions:
        definition = resolved.graph.definitions[text]
        return _Located(node=text, label=qualname(definition.symbol), message="")
    if text in resolved.graph.files:
        return _Located(node=text, label=text, message="")

    candidates = refs.definitions_for(resolved.index, text)
    if not candidates:
        return _Located(node="", label=text, message=f"no symbol or file matches {text}")
    ids = sorted(symbol_stable_id(entry.symbol) for entry in candidates)
    if len(ids) > 1:
        listed = ", ".join(ids)
        return _Located(node="", label=text, message=f"{text} is ambiguous: {listed}")
    return _Located(node=ids[0], label=qualname(candidates[0].symbol), message="")


def _missing(target: str) -> render.Explanation:
    """Build the explanation for a target nothing in the repository defines."""
    return render.Explanation(
        target=target,
        card=None,
        message=f"no symbol matches {target}",
        alternatives=(),
        fan_out=(),
        fan_in=(),
        imports_out=(),
        imports_in=(),
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
) -> tuple[render.ImportRow, ...]:
    """Return the import edges leaving or arriving at one file."""
    matching = [
        edge
        for edge in graph.import_edges()
        if (edge.source.node == path if declared else edge.target.node == path)
    ]
    rows = [
        render.ImportRow(
            path=edge.source.path,
            line=edge.source.line,
            module=edge.name,
            other=edge.target.path,
        )
        for edge in matching
    ]
    return tuple(rows[:limit])


def _trace(
    start: _Located,
    finish: _Located,
    found: resolve.PathResult,
    *,
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
        message=_path_message(found, include_ambiguous=include_ambiguous),
        visited=found.visited,
        exhausted=found.exhausted,
        include_ambiguous=include_ambiguous,
        endpoints_resolved=True,
    )


def _path_message(found: resolve.PathResult, *, include_ambiguous: bool) -> str:
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
    hint = (
        ""
        if include_ambiguous
        else "; name-only-ambiguous edges were excluded, retry including them to widen the search"
    )
    return f"no path from {found.source} to {found.target} over resolved edges{hint}"


def _unresolved_path(
    source: str,
    target: str,
    start: _Located,
    finish: _Located,
    *,
    include_ambiguous: bool,
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
        include_ambiguous=include_ambiguous,
        endpoints_resolved=False,
    )
