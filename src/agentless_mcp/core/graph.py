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

The iteration is hand-rolled (~40 lines) rather than pulling in networkx: the
package's only runtime dependency is the tree-sitter pair, and a power
iteration with an explicit dangling-mass rule is not the part of this tool
worth a dependency. Node order is sorted throughout, so two runs over an
unchanged tree produce bit-identical rankings.
"""

import math
import posixpath
from collections.abc import Collection, Mapping, Sequence, Set
from dataclasses import dataclass, replace
from pathlib import PurePosixPath
from types import MappingProxyType

from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.refs import RefIndex, RepoScan

DEFAULT_DAMPING = 0.85
DEFAULT_EPSILON = 1e-6
DEFAULT_MAX_ITERATIONS = 100

# An import is a declared edge, not an inferred one.
IMPORT_EDGE_WEIGHT = 3.0

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
            segments = _stem(path).split("/")
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
    scan: RepoScan, index: RefIndex, *, stoplist: frozenset[str] = frozenset()
) -> RefGraph:
    """Build the file-level reference graph for one scan."""
    nodes = tuple(sorted(facts.path for facts in scan.files))
    known = frozenset(nodes)
    # Built once for the whole build rather than rebuilt per import name: the
    # path set is the same for every statement in a scan, and the tail search
    # is what used to make this function quadratic in the repository.
    index_of_paths = PathIndex.build(known)
    edges: dict[tuple[str, str], float] = {}

    for facts in scan.files:
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
            contribution = _reference_weight(
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
                key = (facts.path, target)
                edges[key] = edges.get(key, 0.0) + contribution

        for statement in facts.imports:
            for target in _resolved_import_targets(facts.path, statement, index_of_paths):
                if target == facts.path:
                    continue
                key = (facts.path, target)
                edges[key] = edges.get(key, 0.0) + IMPORT_EDGE_WEIGHT

    return RefGraph(nodes=nodes, edges=edges)


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
    """True when ``path``'s extension-less form ends with a whole ``tail``."""
    stem = _stem(path)
    return stem == tail or stem.endswith("/" + tail)


def _stem(path: str) -> str:
    """Return ``path`` without its final extension, in posix form."""
    return PurePosixPath(path).with_suffix("").as_posix()


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
