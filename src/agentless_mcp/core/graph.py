"""File-level relevance graph and personalized PageRank over it.

The map primitive is a ranking, and the ranking is a random walk over "file A
mentions a name file B defines". Three deliberate weightings shape it:

* **Log damping on common names.** A name referenced in fifty files says
  almost nothing about which file matters; a name referenced in two says a
  lot. Every edge contribution is divided by ``1 + log(1 + files
  referencing)``, the aider repo-map treatment of the same problem.
* **A stoplist knob for noise names.** Single- and two-character identifiers
  (``i``, ``x``, ``ok``) collide across every file in a repository, so they
  contribute at a tenth weight rather than being dropped -- dropping them
  would make a file whose only link is a short name unreachable.
* **Weak evidence discounted by tier.** A sole repository-wide definition is
  worth a quarter of a declared relationship; a name with several candidate
  definitions contributes a twentieth to each. These remain useful retrieval
  hints without being allowed to dominate architectural groupings.
* **Import edges weighted 3x.** An import is a declared dependency rather
  than a coincidence of spelling, so it is worth several accidental name
  matches.

Personalization is how ``--focus`` works: seeds take the entire teleport mass,
so rank flows outward from the files the caller named instead of being spread
uniformly. With no seeds the vector is uniform and the result is the plain
PageRank of the repository.

The same graph answers a second, unranked question. :func:`flood` walks out
from a set of files and reports how far each reachable file is, forward or
backward. Backward is the one nothing else here can answer: edges run referrer
to definer, so a test file reaches the code it exercises and that code reaches
nothing back.

The iteration is hand-rolled (~40 lines) rather than pulling in networkx: the
package's only runtime dependency is the tree-sitter pair, and a power
iteration with an explicit dangling-mass rule is not the part of this tool
worth a dependency. Node order is sorted throughout, so two runs over an
unchanged tree produce bit-identical rankings.
"""

import math
import posixpath
from collections.abc import Collection, Iterator, Mapping, Sequence, Set
from dataclasses import dataclass, replace
from pathlib import PurePosixPath
from types import MappingProxyType

from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.refs import FileFacts, RefIndex, RepoScan
from agentless_mcp.core.symbols import base_name

DEFAULT_DAMPING = 0.85
DEFAULT_EPSILON = 1e-6
DEFAULT_MAX_ITERATIONS = 100

# An import is a declared edge, not an inferred one.
IMPORT_EDGE_WEIGHT = 3.0

# The relation-typed weight table, a port of ``CODE_REL_TYPE_WEIGHTS`` in
# code-graph (``src/community/weights.ts``). Read only when ``build_graph`` is
# asked for it; the shipped default weights edges exactly as the paragraphs at
# the top of this module describe.
#
# ``calls`` is in the table and unreachable from here. The file-level graph has
# no call relation to key on -- :class:`~agentless_mcp.core.extractor.Ref`
# records a call and a bare mention as the same ``REFERENCE`` role -- so a call
# is weighted as a reference. The entry stays so the table reads as the ported
# one rather than as a silently trimmed copy.
RELATION_WEIGHTS: Mapping[str, float] = MappingProxyType(
    {
        "inheritance": 3.0,
        "imports": 2.0,
        "calls": 1.5,
        "references": 1.0,
    }
)

# The number of nodes a flood may expand before it stops answering the question
# it was asked. Same value and same reasoning as
# ``agentless_mcp.core.resolve.DEFAULT_MAX_VISITED``, spelled again rather than
# imported: ``resolve`` imports this module, so reading the constant back out
# of it would close a cycle.
DEFAULT_MAX_FLOOD_VISITED = 20_000

# How far a flood walks when the caller names no bound. Ported from
# ``DEFAULT_MAX_DEPTH`` in code-graph's ``src/query/reachability.ts``.
DEFAULT_FLOOD_DEPTH = 20

# Name-only matches are retrieval evidence, not binding evidence. The map
# keeps that recall but discounts it before PageRank and community detection.
UNIQUE_MATCH_MULTIPLIER = 0.25
AMBIGUOUS_MATCH_MULTIPLIER = 0.05

# Names this short collide everywhere; a tenth weight keeps the edge without
# letting loop counters decide what a repository is about.
NOISE_NAME_LENGTH = 2
NOISE_NAME_MULTIPLIER = 0.1

# Extensions tried when turning an import's module string into a repository
# path, ordered so a package's entry point loses to a module of the same name.
_MODULE_SUFFIXES: tuple[str, ...] = (
    ".py",
    ".ts",
    ".tsx",
    ".js",
    ".jsx",
    ".mjs",
    ".cjs",
    ".go",
    ".rb",
    ".rs",
    ".java",
    ".lua",
    ".c",
    ".h",
    ".cpp",
    ".hpp",
    ".sh",
    ".php",
    ".kt",
    ".swift",
    "/__init__.py",
    "/index.ts",
    "/index.tsx",
    "/index.js",
    "",
)

# The file names that make a directory importable under the directory's own
# name. Derived from the entry-point suffixes above so the two cannot drift:
# a suffix added there is answered by the tail search without a second edit.
_PACKAGE_ENTRY_STEMS: frozenset[str] = frozenset(
    PurePosixPath(suffix).with_suffix("").name
    for suffix in _MODULE_SUFFIXES
    if suffix.startswith("/")
)


@dataclass(frozen=True)
class RefGraph:
    """A weighted directed graph over repository-relative file paths.

    Frozen for immutability rather than for hashing: ``edges`` is a mapping,
    so ``hash()`` on a graph raises. Nothing in this package hashes one.
    """

    nodes: tuple[str, ...]
    edges: Mapping[tuple[str, str], float]

    def __post_init__(self) -> None:
        """Refuse a graph the ranking cannot process, at the one place it is built.

        The power iteration walks ``nodes`` and indexes the rank vector by
        edge endpoint, so an edge naming a node outside the graph is not a
        degraded input -- it is an input with no meaning. Refusing both ends
        here is what keeps the two violations one invariant: an unknown source
        used to vanish from the ranking while an unknown target raised
        ``KeyError`` from inside the numeric loop.
        """
        if len(set(self.nodes)) != len(self.nodes):
            repeated = sorted({node for node in self.nodes if self.nodes.count(node) > 1})
            message = f"RefGraph nodes must be distinct: {', '.join(repeated)}"
            raise ValueError(message)

        known = set(self.nodes)
        unknown = sorted({end for edge in self.edges for end in edge if end not in known})
        if unknown:
            message = f"RefGraph edges name nodes outside the graph: {', '.join(unknown)}"
            raise ValueError(message)

        # Copied after validation rather than aliased. The caller still holds
        # the mapping it passed in, and one write to it afterwards adds an
        # edge naming a node this check just refused -- which the ranking then
        # meets as a ``KeyError`` from inside the numeric loop, the exact
        # failure the paragraph above says was eliminated. A validating
        # constructor on a frozen value promises the value cannot become
        # invalid, not that it was valid for one instant.
        object.__setattr__(self, "edges", MappingProxyType(dict(self.edges)))

    def adjacency(self) -> dict[str, tuple[tuple[str, float], ...]]:
        """Return outgoing ``(target, weight)`` pairs per node, in path order.

        Built in one pass over the edge map rather than filtered per node: the
        latter is quadratic in the number of edges, which a repository-sized
        graph feels immediately.
        """
        collected: dict[str, list[tuple[str, float]]] = {node: [] for node in self.nodes}
        for (source, target), weight in self.edges.items():
            collected[source].append((target, weight))
        return {node: tuple(sorted(pairs)) for node, pairs in collected.items()}

    def reverse_adjacency(self) -> dict[str, tuple[tuple[str, float], ...]]:
        """Return incoming ``(source, weight)`` pairs per node, in path order.

        The mirror of :meth:`adjacency`, and built the same single pass over
        the edge map for the same reason: a per-node filter costs one walk of
        every edge per node.

        Edges run referrer to definer, so this is the index that answers "who
        mentions this file". Nothing else in the package can answer it: the
        forward index says what a file reaches and a test file reaches its
        subject while the subject reaches nothing back.
        """
        collected: dict[str, list[tuple[str, float]]] = {node: [] for node in self.nodes}
        for (source, target), weight in self.edges.items():
            collected[target].append((source, weight))
        return {node: tuple(sorted(pairs)) for node, pairs in collected.items()}


@dataclass(frozen=True)
class PathIndex:
    """The repository's known paths, indexed for module-tail matching.

    :func:`resolve_import_target` asks two questions of one path set: is this
    exact candidate a file, and does some file's extension-less path end on a
    separator with this module tail. The second used to be a scan over every
    known path, allocating a ``PurePosixPath`` for each, run once per imported
    name -- so a build cost O(import-names x files) and spent 94% of its time
    there on this package's own tree. The table below answers it with one
    dictionary lookup, and a caller that resolves a repository's worth of
    imports walks the path set once instead of once per name.

    ``by_tail`` already holds the answer :func:`_suffix_match` computes by
    scanning: the shortest path matching each tail, ties broken by path order.
    Storing the winner rather than the candidates keeps the table proportional
    to the repository.

    This is what makes ``src/`` layouts and Go module paths resolve at all:
    ``agentless_mcp.core.refs`` never matches from the repository root, but it
    does match the tail of ``src/agentless_mcp/core/refs.py``.
    """

    paths: frozenset[str]
    by_tail: Mapping[str, str]

    @classmethod
    def build(cls, paths: Collection[str]) -> "PathIndex":
        """Index ``paths`` once, for a caller that will resolve many imports."""
        by_tail: dict[str, str] = {}
        for path in paths:
            for stem in _module_stems(path):
                segments = stem.split("/")
                for start in range(len(segments)):
                    tail = "/".join(segments[start:])
                    held = by_tail.get(tail)
                    if held is None or (len(path), path) < (len(held), held):
                        by_tail[tail] = path
        return cls(paths=frozenset(paths), by_tail=MappingProxyType(by_tail))

    def __contains__(self, path: object) -> bool:
        """True when ``path`` is one of the indexed repository files."""
        return path in self.paths

    def suffix_match(self, module: str) -> str | None:
        """Return the known path this module's tail names, from the table."""
        tail = _module_tail(module)
        return self.by_tail.get(tail) if tail else None


def name_multiplier(name: str, stoplist: frozenset[str] = frozenset()) -> float:
    """Return the per-name weight multiplier: noise names count for less.

    ``stoplist`` is the caller's own list of names that collide in *their*
    repository -- a project-config knob, because no built-in list can know
    that a codebase spells its own ubiquitous helper ``ctx``. Listed names are
    damped exactly like short ones and for the same reason: dropping them
    outright would make a file whose only link is such a name unreachable.
    """
    if len(name) <= NOISE_NAME_LENGTH or name in stoplist:
        return NOISE_NAME_MULTIPLIER
    return 1.0


def common_name_damping(spread: int) -> float:
    """Return the divisor for a name referenced in ``spread`` files.

    One home for the aider treatment of common names, because two views ask
    the same question: the map's edge weights, and the shared-caller
    adjacency, which would otherwise rank a name every file mentions above a
    genuinely shared utility.
    """
    return 1.0 + math.log(1.0 + max(0, spread))


def build_graph(
    scan: RepoScan,
    index: RefIndex,
    *,
    stoplist: frozenset[str] = frozenset(),
    relation_weights: bool = False,
) -> RefGraph:
    """Build the file-level reference graph for one scan.

    ``relation_weights`` swaps the shipped weighting for :data:`RELATION_WEIGHTS`:
    an import edge drops from :data:`IMPORT_EDGE_WEIGHT` to 2.0, a declared base
    class adds an edge of its own at 3.0, and a name reference keeps its damped
    contribution at 1.0. Off by default, and the default is what every caller
    passes today.

    **It is off because it is not language-neutral.** The base classes it reads
    come from ``ASTSymbol.bases``, which exactly one extractor handler fills in
    -- the Python class handler. Every other language records ``bases=()``, so
    turning this on weights Python inheritance and silently weights nothing for
    TypeScript, Go, Java or Rust. Populating ``bases`` for the other class-based
    grammars is the prerequisite for switching it on, not a later improvement.
    """
    nodes = tuple(sorted(facts.path for facts in scan.files))
    known = frozenset(nodes)
    # Built once for the whole build rather than rebuilt per import name: the
    # path set is the same for every statement in a scan, and the tail search
    # is what used to make this function quadratic in the repository.
    index_of_paths = PathIndex.build(known)
    edges: dict[tuple[str, str], float] = {}
    import_weight = RELATION_WEIGHTS["imports"] if relation_weights else IMPORT_EDGE_WEIGHT
    reference_weight = RELATION_WEIGHTS["references"] if relation_weights else 1.0

    for facts in scan.files:
        for target, contribution in _reference_contributions(
            facts, index, stoplist, known, reference_weight
        ):
            key = (facts.path, target)
            edges[key] = edges.get(key, 0.0) + contribution

        for statement in facts.imports:
            for target in _resolved_import_targets(facts.path, statement, index_of_paths):
                if target == facts.path:
                    continue
                key = (facts.path, target)
                edges[key] = edges.get(key, 0.0) + import_weight

        if relation_weights:
            for target in _inheritance_targets(facts, index, known):
                key = (facts.path, target)
                edges[key] = edges.get(key, 0.0) + RELATION_WEIGHTS["inheritance"]

    return RefGraph(nodes=nodes, edges=edges)


def _reference_contributions(
    facts: FileFacts,
    index: RefIndex,
    stoplist: frozenset[str],
    known: frozenset[str],
    reference_weight: float,
) -> Iterator[tuple[str, float]]:
    """Yield one ``(target, contribution)`` pair per name-reference edge.

    Names in sorted order and each name's targets together, so the caller
    accumulates the same floats in the same sequence however it stores them.
    """
    counts: dict[str, int] = {}
    for ref in facts.refs:
        if not ref.is_reference:
            # Bindings, declaration names, labels, and attribute members
            # spell no bare repository relationship.
            continue
        counts[ref.name] = counts.get(ref.name, 0) + 1

    for name in sorted(counts):
        targets = index.defining_paths(name)
        if facts.path in targets:
            # A same-file definition shadows repository-wide name matches.
            continue
        contribution = reference_weight * _reference_weight(
            name,
            counts[name],
            index,
            stoplist,
            candidate_count=len(targets),
        )
        if contribution <= 0.0:
            continue
        for target in targets:
            if target == facts.path or target not in known:
                continue
            yield target, contribution


def _inheritance_targets(facts: FileFacts, index: RefIndex, known: frozenset[str]) -> list[str]:
    """Return the files defining the base classes ``facts`` declares.

    One target per base per declaring symbol, repeats included: a file that
    subclasses three bases out of one module has declared three dependencies on
    it, exactly as three import statements would.

    Only symbols already carrying ``bases`` produce anything, and the Python
    class handler is the only extractor site that fills that field in -- see
    :func:`build_graph` on why the weighting this feeds is off by default.
    """
    targets: list[str] = []
    for symbol in facts.symbols:
        for base in symbol.bases:
            name = base_name(base)
            if not name:
                continue
            defining = index.defining_paths(name)
            if facts.path in defining:
                # A same-file definition shadows repository-wide name matches,
                # the rule the reference pass above applies to every name.
                continue
            targets.extend(
                target for target in defining if target != facts.path and target in known
            )
    return targets


@dataclass(frozen=True)
class PageRank:
    """A ranking, and whether the power iteration behind it finished.

    ``converged`` says why the iteration stopped: the vector moved less than
    ``epsilon`` in a pass, or ``max_iterations`` ran out. A run that hit the
    bound is a partial answer whose tail order can still be wrong, and
    ``iterations`` alone cannot tell the two apart -- a run that settled on
    its hundredth pass reports the same number as one that was cut off at a
    hundred. The exit is reachable at the defaults: measured 2026-08-23, a
    40-node chain at damping 0.99 needs 191 passes to reach an
    ``epsilon`` of 1e-6, against a :data:`DEFAULT_MAX_ITERATIONS` of 100.
    """

    rank: Mapping[str, float]
    iterations: int
    converged: bool


def personalized_pagerank(
    graph: RefGraph,
    seeds: Mapping[str, float] | None = None,
    *,
    damping: float = DEFAULT_DAMPING,
    epsilon: float = DEFAULT_EPSILON,
    max_iterations: int = DEFAULT_MAX_ITERATIONS,
) -> PageRank:
    """Rank the graph's files, teleporting to ``seeds`` instead of uniformly.

    Dangling nodes -- files that reference nothing the repository defines --
    hand their mass to the personalization vector rather than to the uniform
    distribution, which is what makes a focused map stay focused instead of
    leaking rank into every leaf file.

    A run that spends ``max_iterations`` without settling returns the vector
    it reached and says so on :attr:`PageRank.converged`, because a partial
    ranking rendered as a finished one is the failure this package exists to
    prevent.
    """
    nodes = list(graph.nodes)
    if not nodes:
        return PageRank(rank={}, iterations=0, converged=True)

    personalization = _personalization(nodes, seeds)
    adjacency = graph.adjacency()
    totals = {node: sum(weight for _, weight in adjacency[node]) for node in nodes}

    start = 1.0 / len(nodes)
    rank = dict.fromkeys(nodes, start)

    iterations = 0
    converged = False
    while iterations < max_iterations:
        iterations += 1
        incoming = dict.fromkeys(nodes, 0.0)
        dangling = 0.0
        for source in nodes:
            mass = rank[source]
            total = totals[source]
            if total <= 0.0:
                dangling += mass
                continue
            for target, weight in adjacency[source]:
                incoming[target] += mass * weight / total

        updated = {
            node: (1.0 - damping) * personalization[node]
            + damping * (incoming[node] + dangling * personalization[node])
            for node in nodes
        }
        delta = sum(abs(updated[node] - rank[node]) for node in nodes)
        rank = updated
        if delta < epsilon:
            converged = True
            break

    return PageRank(rank=rank, iterations=iterations, converged=converged)


def rank_order(rank: Mapping[str, float]) -> list[str]:
    """Return the ranked paths, highest first, ties broken by path order."""
    return sorted(rank, key=lambda path: (-rank[path], path))


@dataclass(frozen=True)
class Reached:
    """One file a flood arrived at, and the fewest hops that arrive there."""

    path: str
    depth: int


@dataclass(frozen=True)
class Flood:
    """The outcome of one flood: what it reached, and whether it finished.

    ``exhausted`` is the same distinction :class:`PathResult` draws in
    ``agentless_mcp.core.resolve``: a caller must be able to tell "the walk saw
    everything there was" from "the walk stopped looking", because a truncated
    reach set read as a complete one says a file is unrelated when nobody
    checked.
    """

    reached: tuple[Reached, ...]
    visited: int
    exhausted: bool


def flood(
    graph: RefGraph,
    seeds: Collection[str],
    *,
    backward: bool = False,
    max_depth: int = DEFAULT_FLOOD_DEPTH,
    max_visited: int = DEFAULT_MAX_FLOOD_VISITED,
) -> Flood:
    """Walk out from ``seeds`` breadth-first and report every file within reach.

    Forward answers "what do these files reach"; ``backward=True`` answers "what
    reaches these files", which the forward index cannot: edges run referrer to
    definer, so a test file reaches its subject and the subject reaches nothing
    back.

    The seeds themselves are never reported -- a seed is the question, not an
    answer -- and each reported file carries the *fewest* hops that arrive at
    it, so a file two ways away is one row at the shorter distance. Repeat
    seeds, seeds naming no node in the graph, and cycles all fold away in the
    same step: a node is expanded the first time it is seen and never again.

    Rows come back ordered by ``(depth, path)``. The port this follows orders by
    ``(depth, name)``, which is not a total order -- two symbols share a name --
    and here the path *is* the node identity, so the pair is total and two
    floods of an unchanged graph agree row for row.
    """
    known = frozenset(graph.nodes)
    frontier = sorted({seed for seed in seeds if seed in known})
    if not frontier:
        return Flood(reached=(), visited=0, exhausted=False)

    neighbours = graph.reverse_adjacency() if backward else graph.adjacency()
    seen = set(frontier)
    depths: dict[str, int] = {}
    visited = 0
    exhausted = False
    depth = 0

    while frontier and depth < max_depth and not exhausted:
        depth += 1
        following: list[str] = []
        for node in frontier:
            visited += 1
            if visited > max_visited:
                exhausted = True
                break
            for other, _weight in neighbours[node]:
                if other in seen:
                    continue
                seen.add(other)
                depths[other] = depth
                following.append(other)
        frontier = following

    reached = tuple(
        Reached(path=path, depth=hops)
        for path, hops in sorted(depths.items(), key=lambda item: (item[1], item[0]))
    )
    return Flood(reached=reached, visited=visited, exhausted=exhausted)


def resolve_import_target(
    importer: str,
    statement: ImportStatement,
    known_paths: Collection[str] | PathIndex,
) -> str | None:
    """Resolve an import's module string to a file in the repository.

    Best effort by construction: the module string is written for the
    importing language's own resolver, which knows about package roots,
    ``tsconfig`` path maps and vendor directories that this tool does not.
    Unresolved is therefore a normal outcome -- the name-reference edges still
    connect the two files -- and the resolver never guesses between candidates:
    it takes the shortest match so the answer does not depend on walk order.

    This is the one owner of "module string plus importing file becomes a
    repository path": the map's import edges, the resolver's import scopes and
    the patch linter's dependency check all come through here, so a language's
    relative-import spelling is understood in exactly one place.

    ``known_paths`` is membership-tested per candidate, so a caller that
    already holds a set -- both repository-sized callers do -- hands it over
    rather than paying to rebuild one per import statement. A caller
    resolving many imports against one unchanged path set hands over a
    :class:`PathIndex` instead and pays the tail-table walk once for the whole
    run rather than once per imported name.
    """
    module = statement.module.strip()
    relative = _is_relative(module, statement)
    if not module and not relative:
        return None

    index: PathIndex | None
    known: Collection[str]
    if isinstance(known_paths, PathIndex):
        index, known = known_paths, known_paths.paths
    else:
        index = None
        known = known_paths if isinstance(known_paths, Set) else set(known_paths)
    directory = PurePosixPath(importer).parent

    for base in _candidate_bases(module, statement, directory):
        for suffix in _MODULE_SUFFIXES:
            candidate = f"{base}{suffix}" if base else suffix.removeprefix("/")
            if candidate in known:
                return candidate

    if relative:
        # A relative module string is written against the importing file's own
        # directory; matching it against the tail of an unrelated absolute path
        # would be the guess this resolver refuses to make.
        return None
    if index is not None:
        return index.suffix_match(module)
    return _suffix_match(module, known)


def resolve_imported_submodule(
    importer: str,
    statement: ImportStatement,
    name: str,
    known_paths: Collection[str] | PathIndex,
) -> str | None:
    """Resolve ``from package import name`` when ``name`` is a module."""
    dotted = f"{statement.module}.{name}" if statement.module else name
    probe = replace(statement, module=dotted, names=())
    return resolve_import_target(importer, probe, known_paths)


def _resolved_import_targets(
    importer: str,
    statement: ImportStatement,
    known_paths: Collection[str] | PathIndex,
) -> frozenset[str]:
    """Return the repository files one import statement declares."""
    targets = {
        target
        for target in (
            resolve_import_target(importer, statement, known_paths),
            *(
                resolve_imported_submodule(importer, statement, name, known_paths)
                for name in statement.names
            ),
        )
        if target is not None
    }
    return frozenset(targets)


def _is_relative(module: str, statement: ImportStatement) -> bool:
    """True when the import is written against the importing file's directory.

    Two spellings arrive here and only one of them keeps its dots. Python's
    ``from ..pkg import x`` is reported by the extractor as ``module='pkg'``
    with ``relative_level=2`` -- the dots are stripped before the value ever
    gets here -- while a JavaScript ``'../pkg'`` carries no level and the dots
    are all there is to read.
    """
    return bool(statement.relative_level) or module.startswith(".")


def _candidate_bases(
    module: str,
    statement: ImportStatement,
    directory: PurePosixPath,
) -> list[str]:
    """Return the path stems an import could name, most specific first."""
    if statement.relative_level:
        # Python: the level says how far up, and what remains of the module is
        # dotted the same way an absolute one is.
        steps = statement.relative_level - 1
        if steps > len(directory.parts):
            # The level walks above the repository root, which names no file
            # this tool can see. `PurePosixPath(".").parent` is `"."`, so
            # walking up saturates instead of running out, and
            # `from ...... import x` in a top-level module used to resolve
            # against the root as confidently as `from . import x` does.
            return []
        base = directory
        for _ in range(steps):
            base = base.parent
        tail = module.replace(".", "/")
        return [_normalized(base / tail if tail else base)]

    if module.startswith("."):
        # JavaScript-style "./sibling" and "../up/one": the specifier is
        # already a path, dots and separators included.
        return [_normalized(directory / module)]

    if module.endswith(_MODULE_SUFFIXES):
        # The specifier already names a file: C and C++ `#include "money.h"`,
        # shell `source lib/util.sh`. Dotting it the way a Python module
        # string is dotted turns `money.h` into `money/h`, and no suffix
        # appended to that ever matches -- which is why, before this branch,
        # no C or C++ include resolved to a repository file at all and the
        # include graph was empty on every repository. Importer-relative
        # first: `#include "util.h"` names the sibling, not a same-named file
        # at the repository root.
        return [_normalized(directory / module), module]

    dotted = module.replace(".", "/")
    return [dotted, _normalized(directory / dotted)]


def _normalized(path: PurePosixPath) -> str:
    """Collapse ``.`` and ``..`` segments, yielding ``""`` for the repository root.

    ``PurePosixPath`` deliberately never resolves ``..`` -- that would need the
    filesystem -- so a JavaScript ``'../pricing'`` joins to ``src/a/../pricing``
    and matches nothing without this step.
    """
    collapsed = posixpath.normpath(str(path))
    return "" if collapsed == "." else collapsed


def _suffix_match(module: str, known: Collection[str]) -> str | None:
    """Match a module tail against ``known`` by scanning it, with no index.

    The answer :meth:`PathIndex.suffix_match` reads from a table, computed the
    long way for a caller that holds no index. Both exist on purpose: building
    a table costs one walk over the path set, which is the whole saving for a
    caller resolving a repository's worth of imports and a straight loss for a
    caller asking one question. The rule they share -- which tails count and
    which path wins -- lives in :func:`_module_tail`, :func:`_stem` and the
    tie-break spelled the same way in both.
    """
    tail = _module_tail(module)
    if not tail:
        return None

    matches = sorted(
        (path for path in known if _ends_on_boundary(path, tail)),
        key=lambda path: (len(path), path),
    )
    return matches[0] if matches else None


def _ends_on_boundary(path: str, tail: str) -> bool:
    """True when one of ``path``'s module spellings ends with a whole ``tail``."""
    return any(stem == tail or stem.endswith("/" + tail) for stem in _module_stems(path))


def _stem(path: str) -> str:
    """Return ``path`` without its final extension, in posix form."""
    return PurePosixPath(path).with_suffix("").as_posix()


def _module_stems(path: str) -> tuple[str, ...]:
    """Return every module string ``path`` answers to, extension-less.

    One for an ordinary file, two for a package entry point. A package is
    imported by the name of its directory -- `from agentless_mcp.application
    import X` names `application/__init__.py` -- and the tail search compared
    only against the file's own stem, which ends `.../application/__init__`
    and therefore matched nothing. In a `src/` layout the direct-candidate
    loop cannot cover for that: it builds `agentless_mcp/application/__init__.py`
    from the repository root and the file is one directory further down.

    Measured on this repository before the second stem was added: 115 of the
    1199 import statements resolved to nothing, and every one of them named a
    package. Each is an edge missing from the import graph, so a cycle routed
    through a package's `__init__.py` could not be seen at all.
    """
    stem = _stem(path)
    head, separator, last = stem.rpartition("/")
    if separator and last in _PACKAGE_ENTRY_STEMS:
        return (stem, head)
    return (stem,)


def _module_tail(module: str) -> str:
    """Return the slash form of ``module``, or ``""`` when it names no tail.

    The tail must land on a path separator, which is what keeps a match from
    falling inside a component: a bare ``endswith`` lets ``core/refs`` claim
    ``src/mycore/refs.py``, and because the tie-break prefers the shorter
    path it would claim it in preference to the file actually named. A module
    string with no separator at all has no tail to test, so it never matches.
    """
    tail = module.replace(".", "/")
    return tail if "/" in tail else ""


def _reference_weight(
    name: str,
    count: int,
    index: RefIndex,
    stoplist: frozenset[str],
    *,
    candidate_count: int,
) -> float:
    """Weight one file's references to ``name``, damped by how common it is."""
    if candidate_count < 1:
        return 0.0
    # Indexed rather than defaulted: `build_graph` counts the same reference
    # sites `build_ref_index` counts, so every name reaching here is a key. A
    # default would read a mismatched scan and index as "referenced once".
    spread = index.files_referencing[name]
    evidence = UNIQUE_MATCH_MULTIPLIER if candidate_count == 1 else AMBIGUOUS_MATCH_MULTIPLIER
    return count * name_multiplier(name, stoplist) * evidence / common_name_damping(spread)


def _personalization(nodes: Sequence[str], seeds: Mapping[str, float] | None) -> dict[str, float]:
    """Build the teleport vector: normalized seeds, or uniform without them.

    A negative weight is refused rather than clamped to zero. Teleport mass
    has no negative direction, so the number is a caller mistake, and reading
    it as "no seed" hands back a map focused on whichever seeds were spelled
    correctly with nothing said about the one that was not.

    Seeds that name no node in the graph still fall back to the uniform
    vector: that is a focus argument matching nothing, which the callers
    report separately, and an unfocused map is the honest answer to it.
    """
    if seeds:
        negative = sorted(node for node, weight in seeds.items() if weight < 0.0)
        if negative:
            message = f"seed weights must not be negative: {', '.join(negative)}"
            raise ValueError(message)
        weighted = {node: seeds.get(node, 0.0) for node in nodes}
        total = sum(weighted.values())
        if total > 0.0:
            return {node: weight / total for node, weight in weighted.items()}

    share = 1.0 / len(nodes)
    return dict.fromkeys(nodes, share)
