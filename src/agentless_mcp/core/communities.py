"""Deterministic community detection over the file-level reference graph.

A repository map ranks files; a community rollup says which files belong
together. Both read the same graph :mod:`agentless_mcp.core.graph` builds --
nodes are repository-relative paths, edges are name references plus
import-resolved dependencies -- and neither calls a model.

**The algorithm is single-level greedy modularity**, the local-moving phase of
Louvain run to convergence without the graph-aggregation step. The directed
reference graph is symmetrised first (``A = W + W.T``), because "which of these
two files mentions the other" is not the question a rollup answers. Self loops
are dropped; :func:`agentless_mcp.core.graph.build_graph` never emits one, and
carrying the two competing self-loop conventions through the modularity
arithmetic would buy nothing.

The aggregation step is deliberately not here, and that is a measurement
rather than a preference. A reference graph is dense, and on a dense graph the
second Louvain level does exactly what a rollup must not do: it merges.
Measured 2026-08-23 over this package's own tree of 161 files and 778 edges,
average degree 9.7, by running one level, collapsing the graph onto it and
running a second level: at resolution 1.0 the second level merged 36
communities into 22 whose three largest held 43, 39 and 31 of the 161 files;
at resolution 0.5 it put 113 of the 161 in one community. The second level
scores slightly *higher* on the original graph (Q 0.329 -> 0.341), and that is
the finding rather than an argument against it -- modularity rewards the
merge, so the score cannot be the thing that decides. A rollup whose largest
group is a quarter of the repository has stopped answering "which files belong
together".

One level at resolution 1.0 gave Q = 0.329 over 36 communities on that tree,
labelled by directory paths such as ``tests/unit``. That score is modest, and
it is the honest score of a graph where every file shares identifier names
with most others; the partition is neither a blob nor all singletons, and its
labels are the directory structure. Callers wanting a coarser rollup should
lower ``resolution``, which buys the same merge under the caller's control,
rather than add a level. Scores found at two resolutions are not comparable
-- see :class:`CommunityPartition`.

**Determinism is a property of three explicit rules**, and every one of them is
an ordering decision that would otherwise be taken by dictionary iteration:

1. *Nodes are visited in sorted path order*, on every pass. The result of a
   greedy pass depends on visit order, so the visit order is fixed by the data
   rather than by the order the graph happened to be built in.
2. *A community's identity is the index of its founding node* in that sorted
   order, so community 0 is always the community of the lexicographically
   first path, whatever it grows into.
3. *A tie leaves the node where it is.* A move happens only when some other
   community scores strictly better -- by more than :data:`EPSILON` -- than the
   node's current one, and among candidates that clear that bar the first one
   in ascending community-id order wins. Equal-gain moves are therefore never
   taken, which is what removes both the dictionary-order dependency and the
   two-node oscillation that an "at least as good" rule permits.

Identical input produces identical communities, pinned by a test.

Labels are mechanical, never generated: the label of a community is the
deepest directory prefix shared by a strict majority of its members, and
:data:`ROOT_LABEL` when no prefix reaches a majority. Depth alone decides,
because a majority at one depth can only be held by one prefix. A label is
therefore
repository content, and every renderer that shows one has to treat it as
untrusted -- see :func:`agentless_mcp.core.mermaid.safe_label`.
"""

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any

from agentless_mcp.core.graph import RefGraph

# The resolution knob of the modularity objective. Above 1.0 the partition
# breaks into more, smaller communities; below it, into fewer, larger ones.
DEFAULT_RESOLUTION = 1.0

# Local moving converges in a handful of passes on any graph this tool sees;
# the bound exists so that a pathological graph cannot spin, not as a tuning
# parameter.
DEFAULT_MAX_PASSES = 50

# Gains are differences of sums of floats, so "strictly better" needs a floor
# that is above the arithmetic's own noise and far below any real gain.
EPSILON = 1e-12

# What a community of top-level files with no shared directory is called.
# Deliberately free of characters a diagram renderer would have to escape.
ROOT_LABEL = "repository root"


@dataclass(frozen=True)
class Community:
    """One community: its members, its mechanical label and its weights.

    ``internal_weight`` is the summed weight of edges with both endpoints
    inside the community and ``total_weight`` the summed degree of its
    members, both on the symmetrised graph. They are the evidence behind the
    partition's modularity, carried so a caller can tell a genuinely cohesive
    group from a residue of unconnected files that the algorithm had nowhere
    better to put.
    """

    label: str
    members: tuple[str, ...]
    internal_weight: float
    total_weight: float

    @property
    def size(self) -> int:
        """The number of files in this community."""
        return len(self.members)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this community."""
        return {
            "label": self.label,
            "size": self.size,
            "members": list(self.members),
            "internal_weight": self.internal_weight,
            "total_weight": self.total_weight,
        }


@dataclass(frozen=True)
class CommunityPartition:
    """A whole partition: the communities, and how good it is.

    ``modularity`` is the score of this partition under the resolution it was
    found at, **and it is comparable only to another score found at the same
    resolution**. The published reading -- roughly 0.3 and up is a repository
    with real module boundaries, near 0 means the detector found nothing and
    split the files arbitrarily -- is calibrated for ``resolution == 1.0``
    alone. :func:`_partition` reports the resolution-scaled generalized
    modularity, so lowering the knob lowers the null-model term the score
    subtracts and raises the score for an unchanged tree: measured 2026-08-23
    on this package, 0.732 at resolution 0.25 against 0.131 at 4.0, a spread
    of more than five times with nothing about the repository changed. A
    caller lowering the resolution for a coarser rollup, which this module
    recommends, is not being told the repository gained structure.

    ``converged`` says why local moving stopped: no node wanted to move, or
    ``max_passes`` ran out. A partition that hit the bound is a partial answer,
    and ``passes`` alone cannot tell the two apart -- a run that converged on
    its fiftieth pass reports the same number as one that was cut off at
    fifty.
    """

    communities: tuple[Community, ...]
    modularity: float
    resolution: float
    passes: int
    converged: bool

    def index_of(self) -> dict[str, int]:
        """Map each member path to the position of its community."""
        return {
            member: position
            for position, community in enumerate(self.communities)
            for member in community.members
        }

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this partition."""
        return {
            "modularity": self.modularity,
            "resolution": self.resolution,
            "passes": self.passes,
            "converged": self.converged,
            "communities": [community.as_dict() for community in self.communities],
        }


@dataclass(frozen=True)
class _Weighted:
    """The symmetrised graph the modularity arithmetic runs on.

    One value rather than three parallel arguments, because ``adjacency``,
    ``degree`` and ``two_m`` are derived from each other in one step and a
    caller holding a mismatched trio would compute a meaningless score.
    """

    adjacency: Mapping[str, Mapping[str, float]]
    degree: Mapping[str, float]
    two_m: float


def detect_communities(
    graph: RefGraph,
    *,
    resolution: float = DEFAULT_RESOLUTION,
    max_passes: int = DEFAULT_MAX_PASSES,
) -> CommunityPartition:
    """Partition ``graph`` into communities, largest first.

    Communities are ordered by size and ties are broken by member order, so
    two runs over the same graph return the same list in the same order and a
    caller may cite "community 0" meaningfully.

    A graph with no edges is not a failure and is not forced together: every
    node becomes its own community and the modularity is 0.0, which is the
    true score of that partition rather than a placeholder.

    A run that spends ``max_passes`` without settling returns what it reached
    and says so on :attr:`CommunityPartition.converged`, because a partial
    partition an agent reads as final is worse than a slow one.
    """
    nodes = sorted(set(graph.nodes))
    if not nodes:
        return CommunityPartition(
            communities=(),
            modularity=0.0,
            resolution=resolution,
            passes=0,
            converged=True,
        )

    weighted = _symmetrise(graph, nodes)
    membership = {node: index for index, node in enumerate(nodes)}
    if weighted.two_m <= 0.0:
        return _partition(membership, weighted, resolution=resolution, passes=0, converged=True)

    total = {index: weighted.degree[node] for index, node in enumerate(nodes)}
    passes = 0
    converged = False
    while passes < max_passes:
        passes += 1
        if not _one_pass(nodes, weighted, total, membership, resolution):
            converged = True
            break

    return _partition(
        membership, weighted, resolution=resolution, passes=passes, converged=converged
    )


def community_label(members: Sequence[str]) -> str:
    """Return the mechanical label for a set of member paths.

    The deepest directory prefix shared by a strict majority of the members:
    deep enough to say ``src/agentless_mcp/core`` rather than ``src``,
    tolerant enough that one outlier file does not collapse the label to
    nothing, and strict enough that a community split evenly between two
    directories is labelled by the parent they share rather than by whichever
    half sorts first.

    Depth alone picks the winner. :func:`_directory_prefixes` gives each member
    exactly one prefix per depth, so the counts at one depth sum to at most
    ``len(members)`` and two prefixes there cannot both clear a strict
    majority. The prefix in the sort key below therefore decides nothing the
    data can produce; it is kept so that a hand-built call reaching this
    function with counts the rule above cannot yield still answers the same way
    twice, rather than by dictionary order.
    """
    if not members:
        return ROOT_LABEL

    counts: dict[str, int] = {}
    for member in members:
        for prefix in _directory_prefixes(member):
            counts[prefix] = counts.get(prefix, 0) + 1

    threshold = len(members) // 2 + 1
    eligible = [prefix for prefix, count in counts.items() if count >= threshold]
    if not eligible:
        return ROOT_LABEL
    return min(eligible, key=lambda prefix: (-prefix.count("/"), prefix))


def _directory_prefixes(path: str) -> list[str]:
    """Return every directory prefix of ``path``, shallowest first."""
    parts = path.split("/")
    return ["/".join(parts[:depth]) for depth in range(1, len(parts))]


def _symmetrise(graph: RefGraph, nodes: Sequence[str]) -> _Weighted:
    """Return the undirected weighted view of ``graph`` over ``nodes``.

    Self loops and non-positive weights are dropped: neither has a meaning in
    the modularity objective, and keeping them would only make the arithmetic
    answer a question nobody asked. A hand-built graph can still supply both,
    which is why the two clauses stay.

    Endpoints are not re-checked.
    :meth:`agentless_mcp.core.graph.RefGraph.__post_init__` refuses an edge
    naming a node the graph does not list, and ``nodes`` is that node list, so
    a membership test here would be a second home for an invariant the value
    object already owns -- and a dead one, which reads as a live hazard to
    everyone after.
    """
    adjacency: dict[str, dict[str, float]] = {node: {} for node in nodes}
    for (source, target), weight in graph.edges.items():
        if source == target or weight <= 0.0:
            continue
        adjacency[source][target] = adjacency[source].get(target, 0.0) + weight
        adjacency[target][source] = adjacency[target].get(source, 0.0) + weight

    degree = {node: sum(adjacency[node].values()) for node in nodes}
    return _Weighted(adjacency=adjacency, degree=degree, two_m=sum(degree.values()))


def _one_pass(
    nodes: Sequence[str],
    weighted: _Weighted,
    total: dict[int, float],
    membership: dict[str, int],
    resolution: float,
) -> int:
    """Move each node to its best community once; return how many moved.

    The score of putting node ``i`` into community ``c`` is
    ``w(i, c) - resolution * k(i) * tot(c) / 2m``, the modularity gain with
    the positive constant factor divided out. The node is removed from its own
    community before any candidate is scored, which is what lets the community
    it came from be evaluated on the same terms as the others.
    """
    moved = 0
    for node in nodes:
        origin = membership[node]
        node_degree = weighted.degree[node]
        total[origin] -= node_degree

        weights: dict[int, float] = {origin: 0.0}
        for neighbour, weight in weighted.adjacency[node].items():
            community = membership[neighbour]
            weights[community] = weights.get(community, 0.0) + weight

        best = origin
        best_score = _score(weights[origin], node_degree, total[origin], weighted, resolution)
        for community in sorted(weights):
            score = _score(weights[community], node_degree, total[community], weighted, resolution)
            if score > best_score + EPSILON:
                best_score = score
                best = community

        total[best] += node_degree
        membership[node] = best
        if best != origin:
            moved += 1
    return moved


def _score(
    into_community: float,
    node_degree: float,
    community_total: float,
    weighted: _Weighted,
    resolution: float,
) -> float:
    """Return the modularity gain of one candidate move, up to a constant."""
    return into_community - resolution * node_degree * community_total / weighted.two_m


def _partition(
    membership: Mapping[str, int],
    weighted: _Weighted,
    *,
    resolution: float,
    passes: int,
    converged: bool,
) -> CommunityPartition:
    """Turn a membership map into the ordered, labelled, scored partition."""
    grouped: dict[int, list[str]] = {}
    for node in sorted(membership):
        grouped.setdefault(membership[node], []).append(node)

    communities: list[Community] = []
    modularity = 0.0
    for community_id in sorted(grouped):
        members = tuple(grouped[community_id])
        internal = sum(
            weight
            for member in members
            for neighbour, weight in weighted.adjacency[member].items()
            if membership[neighbour] == community_id
        )
        outgoing = sum(weighted.degree[member] for member in members)
        if weighted.two_m > 0.0:
            modularity += internal / weighted.two_m - resolution * (outgoing / weighted.two_m) ** 2
        communities.append(
            Community(
                label=community_label(members),
                members=members,
                internal_weight=internal,
                total_weight=outgoing,
            )
        )

    communities.sort(key=lambda community: (-community.size, community.members))
    return CommunityPartition(
        communities=tuple(communities),
        modularity=modularity,
        resolution=resolution,
        passes=passes,
        converged=converged,
    )
