"""File-level reference graph and personalized PageRank over it.

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
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import PurePosixPath

from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.refs import RefIndex, RepoScan

DEFAULT_DAMPING = 0.85
DEFAULT_EPSILON = 1e-6
DEFAULT_MAX_ITERATIONS = 100

# An import is a declared edge, not an inferred one.
IMPORT_EDGE_WEIGHT = 3.0

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
    """A weighted directed graph over repository-relative file paths."""

    nodes: tuple[str, ...]
    edges: Mapping[tuple[str, str], float]

    def adjacency(self) -> dict[str, tuple[tuple[str, float], ...]]:
        """Return outgoing ``(target, weight)`` pairs per node, in path order.

        Built in one pass over the edge map rather than filtered per node: the
        latter is quadratic in the number of edges, which a repository-sized
        graph feels immediately.
        """
        collected: dict[str, list[tuple[str, float]]] = {node: [] for node in self.nodes}
        for (source, target), weight in self.edges.items():
            collected.setdefault(source, []).append((target, weight))
        return {node: tuple(sorted(pairs)) for node, pairs in collected.items()}


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
    known = set(nodes)
    edges: dict[tuple[str, str], float] = {}

    for facts in scan.files:
        counts: dict[str, int] = {}
        for ref in facts.refs:
            counts[ref.name] = counts.get(ref.name, 0) + 1

        for name in sorted(counts):
            contribution = _reference_weight(name, counts[name], index, stoplist)
            if contribution <= 0.0:
                continue
            for target in index.defining_paths(name):
                if target == facts.path or target not in known:
                    continue
                key = (facts.path, target)
                edges[key] = edges.get(key, 0.0) + contribution

        for statement in facts.imports:
            imported = resolve_import_target(facts.path, statement, nodes)
            if imported is None or imported == facts.path:
                continue
            key = (facts.path, imported)
            edges[key] = edges.get(key, 0.0) + IMPORT_EDGE_WEIGHT

    return RefGraph(nodes=nodes, edges=edges)


def personalized_pagerank(
    graph: RefGraph,
    seeds: Mapping[str, float] | None = None,
    *,
    damping: float = DEFAULT_DAMPING,
    epsilon: float = DEFAULT_EPSILON,
    max_iterations: int = DEFAULT_MAX_ITERATIONS,
) -> dict[str, float]:
    """Rank the graph's files, teleporting to ``seeds`` instead of uniformly.

    Dangling nodes -- files that reference nothing the repository defines --
    hand their mass to the personalization vector rather than to the uniform
    distribution, which is what makes a focused map stay focused instead of
    leaking rank into every leaf file.
    """
    nodes = list(graph.nodes)
    if not nodes:
        return {}

    personalization = _personalization(nodes, seeds)
    adjacency = graph.adjacency()
    totals = {node: sum(weight for _, weight in adjacency[node]) for node in nodes}

    start = 1.0 / len(nodes)
    rank = dict.fromkeys(nodes, start)

    for _ in range(max_iterations):
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
            break

    return rank


def rank_order(rank: Mapping[str, float]) -> list[str]:
    """Return the ranked paths, highest first, ties broken by path order."""
    return sorted(rank, key=lambda path: (-rank[path], path))


def resolve_import_target(
    importer: str,
    statement: ImportStatement,
    known_paths: Sequence[str],
) -> str | None:
    """Resolve an import's module string to a file in the repository.

    Best effort by construction: the module string is written for the
    importing language's own resolver, which knows about package roots,
    ``tsconfig`` path maps and vendor directories that this tool does not.
    Unresolved is therefore a normal outcome -- the name-reference edges still
    connect the two files -- and the resolver never guesses between candidates:
    it takes the shortest match so the answer does not depend on walk order.
    """
    module = statement.module.strip()
    if not module:
        return None

    known = set(known_paths)
    directory = PurePosixPath(importer).parent

    for base in _candidate_bases(module, statement, directory):
        for suffix in _MODULE_SUFFIXES:
            candidate = f"{base}{suffix}"
            if candidate in known:
                return candidate

    return _suffix_match(module, known)


def _candidate_bases(
    module: str,
    statement: ImportStatement,
    directory: PurePosixPath,
) -> list[str]:
    """Return the path stems an import could name, most specific first."""
    if module.startswith("."):
        # JavaScript-style "./sibling" and "../up/one", and Python-style
        # ".module" / "..package.module", which the extractor reports with an
        # explicit relative_level.
        if statement.relative_level:
            base = directory
            for _ in range(statement.relative_level - 1):
                base = base.parent
            tail = module.lstrip(".").replace(".", "/")
            return [str(base / tail) if tail else str(base)]
        cleaned = PurePosixPath(module)
        return [str((directory / cleaned).as_posix()).replace("/./", "/")]

    dotted = module.replace(".", "/")
    return [dotted, str(directory / dotted)]


def _suffix_match(module: str, known: set[str]) -> str | None:
    """Match a dotted or slashed module against the tail of a known path.

    This is what makes ``src/`` layouts and Go module paths resolve at all:
    ``agentless_mcp.core.refs`` never matches from the repository root, but it
    does match the tail of ``src/agentless_mcp/core/refs.py``.
    """
    tail = module.replace(".", "/")
    if "/" not in tail:
        return None

    matches = sorted(
        (path for path in known if PurePosixPath(path).with_suffix("").as_posix().endswith(tail)),
        key=lambda path: (len(path), path),
    )
    return matches[0] if matches else None


def _reference_weight(name: str, count: int, index: RefIndex, stoplist: frozenset[str]) -> float:
    """Weight one file's references to ``name``, damped by how common it is."""
    spread = index.files_referencing.get(name, 1)
    return count * name_multiplier(name, stoplist) / common_name_damping(spread)


def _personalization(nodes: Sequence[str], seeds: Mapping[str, float] | None) -> dict[str, float]:
    """Build the teleport vector: normalized seeds, or uniform without them."""
    if seeds:
        weighted = {node: max(0.0, seeds.get(node, 0.0)) for node in nodes}
        total = sum(weighted.values())
        if total > 0.0:
            return {node: weight / total for node, weight in weighted.items()}

    share = 1.0 / len(nodes)
    return dict.fromkeys(nodes, share)
