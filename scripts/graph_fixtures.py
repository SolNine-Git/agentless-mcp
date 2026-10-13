"""Write the graph test fixtures: each corpus's facts and the Python graph outputs for them.

Development-time only. Run it from the repository root, with every cache in a scratch directory:

    XDG_CACHE_HOME=<scratch> TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch grammar libraries> \
        uv run python scripts/graph_fixtures.py

A corpus is one repository scan: each fixture repository under tests/characterization/fixtures/,
that whole tree as one scan, and the generated repositories in EXTENSIONS. Each corpus file holds
the scan's facts in the serde form of the facts crate, then the reference outputs: the name index,
the import scopes, the file graph, the ranks, the floods, the communities, the symbol graph, the
cycles, the paths, the resolver tiers, the lookups and the line owners. units.json holds the path,
import and weight rules over chosen inputs. Each float is stored as its bits.

Python orders the import targets of one statement by hash (graph.py:795-806), and that order sets
the insertion order of the file graph's edges. The script replays the insertion order with the
targets of each statement in first-occurrence order, which is the canonical order of the port, and
takes every weight from Python. The ranks and communities run on a graph in that order. With
--check, the script compares its output with the committed fixtures and writes nothing.
"""

import os
import posixpath
import struct
import sys
import tempfile
from collections.abc import Iterable, Sequence
from pathlib import Path, PurePosixPath

from fixture_files import (
    GRAMMAR_OVERRIDE,
    Fixture,
    refuse_real_caches,
    render,
    write_or_check_files,
)

from agentless_mcp.application.map_service import TEST_COMPANION_DEPTH
from agentless_mcp.application.symbol_service import is_test_path
from agentless_mcp.core import communities, graph, refs, resolve
from agentless_mcp.core.extractor import Ref, TreeSitterExtractor
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.slices import span_end
from agentless_mcp.core.symbols import ASTSymbol, Rationale, id_qualname, qualname, symbol_stable_id

ROOT = Path(__file__).resolve().parents[1]
CORPUS = ROOT / "tests" / "characterization" / "fixtures"
FIXTURES = ROOT / "crates" / "graph" / "fixtures"
REPOSITORIES = (
    "repo_go",
    "repo_nested",
    "repo_py",
    "repo_py_tests",
    "repo_ts",
    "shapes",
    "tier1",
    "tier2",
)
TREE = "fixtures_tree"
# The whole tree holds the files of the repositories above, so it keeps only the sections that
# change across repositories, and its file stays under the 1000 KB pre-commit cap.
TREE_SECTIONS: dict[str, tuple[str, ...]] = {
    TREE: (
        "files",
        "file_graph",
        "pagerank",
        "flood",
        "communities",
        "scopes",
        "symbol_edges",
        "symbol_graph",
        "ambiguous",
        "candidates",
        "import_edges",
        "import_graph",
        "cycles",
        "paths",
        "sites",
        "through_imports",
    )
}
STOPLIST = frozenset({"helper", "price", "render", "total"})
WEIGHTINGS: dict[str, tuple[frozenset[str], bool]] = {
    "default": (frozenset(), False),
    "stoplist": (STOPLIST, False),
    "relation_weights": (frozenset(), True),
}
RESOLUTIONS = (1.0, 0.25, 4.0)
POLICIES = ((False, False), (True, False), (False, True), (True, True))
SAMPLED_NODES = 8
SMALL_BOUNDS = (1, 2, 5)
BOUNDED_PAIRS = 5
RESOLVE_GRID_FILES = 20
MISSING = "missing/nowhere.py"

Case = dict[str, object]
Sources = dict[str, str]
EdgeOrder = dict[tuple[str, str], None]
EdgePosition = dict[resolve.SymbolEdge, int]
EdgeGroups = dict[str, tuple[resolve.SymbolEdge, ...]]
Grouped = tuple[EdgeGroups, EdgeGroups]

EXT_PYTHON: Sources = {
    "pkg/__init__.py": (
        "from .core import *\nfrom .helpers import helper as public_helper\n\nVERSION = 1\n"
    ),
    "pkg/core.py": (
        "from pkg import helpers\n"
        "from . import cycle_a\n"
        "import pkg.models as models\n"
        "\n"
        "\n"
        "def run(order):\n"
        "    total = helpers.helper(order)\n"
        "    item = models.Item()\n"
        "    return cycle_a.start(total, item)\n"
        "\n"
        "\n"
        "def _private():\n"
        "    return render(0)\n"
        "\n"
        "\n"
        "class Engine(models.Base):\n"
        "    def run(self):\n"
        "        return run(self)\n"
    ),
    "pkg/helpers.py": (
        "from .models import Item as Thing\n"
        "\n"
        "\n"
        "def helper(order):\n"
        "    ok = Thing()\n"
        "    return ok\n"
        "\n"
        "\n"
        "def render(value):\n"
        "    return value\n"
    ),
    "pkg/models.py": (
        "class Base:\n"
        "    def save(self):\n"
        "        return 1\n"
        "\n"
        "\n"
        "class Item(Base):\n"
        "    def save(self):\n"
        "        return render(self)\n"
        "\n"
        "\n"
        "def render(item):\n"
        "    return str(item)\n"
    ),
    "pkg/cycle_a.py": (
        "from pkg.cycle_b import step\n\n\ndef start(total, item):\n    return step(total)\n"
    ),
    "pkg/cycle_b.py": (
        "from pkg.cycle_c import finish\n\n\ndef step(total):\n    return finish(total)\n"
    ),
    "pkg/cycle_c.py": (
        "from pkg import cycle_a\nfrom pkg import cycle_b\n\n\n"
        "def finish(total):\n    return cycle_a.start(total, None)\n"
    ),
    "pkg/sub/__init__.py": "from ..models import Base\n",
    "pkg/sub/deep.py": (
        "from ...pkg import models\n"
        "from .. import helpers\n"
        "from ....far import away\n"
        "import os.path\n"
        "import pkg.sub.deep\n"
        "from pkg.sub import Base\n"
        "\n"
        "\n"
        "class Deep(Base, metaclass=type):\n"
        "    def save(self):\n"
        "        return helpers.helper(models.Item())\n"
        "\n"
        "\n"
        "class Deeper(Deep):\n"
        "    pass\n"
    ),
    "pair_a.py": "import pair_b\n\n\ndef a():\n    return pair_b.b()\n",
    "pair_b.py": "import pair_a\n\n\ndef b():\n    return pair_a.a()\n",
    "tools/dup.py": (
        "def render(x):\n"
        "    return x\n"
        "\n"
        "\n"
        "def render(x, y):\n"
        "    return y\n"
        "\n"
        "\n"
        "def f():\n"
        "    return ctx()\n"
        "\n"
        "\n"
        "def ctx():\n"
        "    return f()\n"
        "\n"
        "\n"
        "i = 0\n"
        "price = render(i)\n"
    ),
    "tests/test_core.py": (
        "from pkg.core import run, Engine\n"
        "from pkg import helpers as h\n"
        "\n"
        "\n"
        "def test_run():\n"
        "    assert run(1)\n"
        "    h.helper(2)\n"
        "    Engine().run()\n"
        "\n"
        "\n"
        "render(3)\n"
    ),
    "tests/conftest.py": "from pkg.models import Item\n\n\ndef item():\n    return Item()\n",
    "config/settings.json": '{"render": 1, "run": {"helper": 2}}\n',
    "config/app.yaml": "run: 1\nhelper:\n  render: 2\n",
    "main.py": (
        "from pkg import *\n"
        "from pkg.core import run as go\n"
        "import pkg\n"
        "import pkg.core\n"
        "\n"
        "pkg.core.run(1)\n"
        "go(1)\n"
        "render(2)\n"
        "print(VERSION, public_helper)\n"
    ),
}

EXT_SCRIPTS: Sources = {
    "web/src/index.ts": (
        "export * from './util';\n"
        "import { a as b } from './lib/math';\n"
        "import shared from '../shared';\n"
        "export function main(): number {\n"
        "  return b() + shared();\n"
        "}\n"
    ),
    "web/src/util.ts": "export function a(): number {\n  return 1;\n}\nexport const util = 1;\n",
    "web/src/lib/math.ts": (
        "import { util } from '../util';\nexport function a(): number {\n  return util;\n}\n"
    ),
    "web/shared/index.js": "function shared() {\n  return 2;\n}\nmodule.exports = shared;\n",
    "web/src/app.tsx": (
        "import { util } from './util';\n"
        "import * as M from './lib/math';\n"
        "export function App() {\n"
        "  return <div>{M.a() + util}</div>;\n"
        "}\n"
    ),
    "native/util.h": "int add(int a, int b);\n",
    "native/util.c": '#include "util.h"\n\nint add(int a, int b) {\n    return a + b;\n}\n',
    "native/main.c": (
        '#include "util.h"\n#include <stdio.h>\n#include "missing.h"\n#include "util.h"\n\n'
        "int main(void) {\n    return add(1, 2);\n}\n"
    ),
    "util.h": "int root_add(int a);\n",
    "scripts/run.sh": "source lib/common.sh\n. ./lib/common.sh\ncommon\n",
    "scripts/lib/common.sh": "common() {\n  echo hi\n}\n",
    "lua/init.lua": 'local m = require("lua.mod")\nm.go()\n',
    "lua/mod.lua": "local M = {}\nfunction M.go()\n  return 1\nend\nreturn M\n",
}


def cluster(group: str, size: int, links: dict[int, Sequence[str]]) -> Sources:
    """Return ``size`` modules in ``group``, each importing up to three others, plus ``links``."""
    files: Sources = {f"{group}/__init__.py": ""}
    for number in range(size):
        others = [other for other in range(size) if other != number][:3]
        imports = [f"from {group} import m{other}\n" for other in others]
        imports += [f"import {module}\n" for module in links.get(number, ())]
        body = f"\n\ndef f{group}{number}():\n    return {number}\n"
        files[f"{group}/m{number}.py"] = "".join(imports) + body
    return files


EXT_COMMUNITIES: Sources = {
    **cluster("alpha", 5, {0: ("beta.m0",)}),
    **cluster("beta", 5, {1: ("alpha.m1",)}),
    **cluster("delta", 4, {}),
    "gamma/lonely.py": "def lonely():\n    return 0\n",
    "solo.py": "def solo():\n    return 0\n",
    "hub.py": (
        "import alpha.m2\nimport beta.m2\nimport delta.m2\nimport alpha.m3\n\n\n"
        "def hub():\n    return 0\n"
    ),
}

EXTENSIONS: dict[str, Sources] = {
    "ext_communities": EXT_COMMUNITIES,
    "ext_python": EXT_PYTHON,
    "ext_scripts": EXT_SCRIPTS,
}


def bits(value: float) -> str:
    """Return the IEEE 754 bits of ``value`` as 0x and 16 hex digits."""
    return f"0x{struct.unpack('<Q', struct.pack('<d', value))[0]:016x}"


def rationale_value(rationale: Rationale) -> Case:
    return {
        "kind": rationale.kind,
        "text": rationale.text,
        "line": rationale.line_number,
        "citations": list(rationale.citations),
        "duplicate_index": rationale.duplicate_index,
    }


def symbol_value(symbol: ASTSymbol) -> Case:
    return {
        "name": symbol.name,
        "kind": symbol.kind.value,
        "line": symbol.line_number,
        "end_line": span_end(symbol),
        "signature": symbol.signature,
        "parent": symbol.parent_class,
        "bases": list(symbol.bases),
        "is_public": symbol.is_public,
        "rationales": [rationale_value(rationale) for rationale in symbol.rationales],
        "duplicate_index": symbol.duplicate_index,
    }


def import_value(statement: ImportStatement) -> Case:
    return {
        "module": statement.module,
        "names": list(statement.names),
        "local_names": list(statement.local_names),
        "is_relative": statement.is_relative,
        "relative_level": statement.relative_level,
        "line": statement.line_number,
        "binds_all": statement.binds_all,
        "alias": statement.alias,
    }


def ref_value(ref: Ref) -> Case:
    return {"name": ref.name, "line": ref.line, "role": ref.role.value, "qualifier": ref.qualifier}


def file_case(facts: refs.FileFacts) -> Case:
    return {
        "path": facts.path,
        "language": facts.language,
        "line_count": facts.line_count,
        "facts": {
            "symbols": [symbol_value(symbol) for symbol in facts.symbols],
            "imports": [import_value(statement) for statement in facts.imports],
            "refs": [ref_value(ref) for ref in facts.refs],
            "errors": [],
        },
    }


def endpoint_value(endpoint: resolve.Endpoint) -> list[object]:
    return [endpoint.node, endpoint.path, endpoint.line, endpoint.label, endpoint.is_symbol]


def edge_value(edge: resolve.SymbolEdge) -> list[object]:
    return [
        endpoint_value(edge.source),
        endpoint_value(edge.target),
        edge.name,
        edge.relation.value,
        edge.tier.value,
    ]


def reference_value(reference: resolve.AmbiguousReference) -> list[object]:
    return [endpoint_value(reference.source), reference.name, reference.relation.value]


def ids(definitions: Iterable[refs.Definition]) -> list[str]:
    return [symbol_stable_id(definition.symbol) for definition in definitions]


def name_case(index: refs.RefIndex, name: str) -> Case:
    sites = index.sites.get(name, ())
    return {
        "name": name,
        "definitions": [
            [entry.path, symbol_stable_id(entry.symbol)]
            for entry in index.definitions.get(name, ())
        ],
        "sites": [[site.path, site.line, site.role.value] for site in sites],
        "defining_paths": list(index.defining_paths(name)),
        "reference_spread": index.files_referencing.get(name),
        "caller_spread": index.files_referencing.get(name, 1),
        "spelling_spread": len({site.path for site in sites}),
    }


def scope_case(path: str, scope: resolve.ImportScope) -> Case:
    return {
        "path": path,
        "wholesale": sorted(scope.wholesale),
        "module_bindings": [
            [name, sorted(paths)] for name, paths in sorted(scope.module_bindings.items())
        ],
        "named": [[name, sorted(paths)] for name, paths in sorted(scope.named.items())],
        "members": [
            [name, sorted(map(list, pairs))] for name, pairs in sorted(scope.members.items())
        ],
        "statements": [
            [row.module, row.line, row.resolved, row.relative] for row in scope.statements
        ],
        "resolved_edges": [list(edge) for edge in scope.resolved_edges(path)],
    }


def statement_targets(
    importer: str, statement: ImportStatement, paths: graph.PathIndex
) -> list[str]:
    """Return one statement's targets in first-occurrence order: its own, then each name's."""
    found = [
        graph.resolve_import_target(importer, statement, paths),
        *(
            graph.resolve_imported_submodule(importer, statement, name, paths)
            for name in statement.names
        ),
    ]
    return list(dict.fromkeys(target for target in found if target is not None))


def import_keys(facts: refs.FileFacts, paths: graph.PathIndex, order: EdgeOrder) -> int:
    """Add one file's import edges to ``order``; return the statements that added two or more."""
    seeded = 0
    for statement in facts.imports:
        fresh = [
            (facts.path, target)
            for target in statement_targets(facts.path, statement, paths)
            if target != facts.path and (facts.path, target) not in order
        ]
        order.update(dict.fromkeys(fresh))
        seeded += len(fresh) > 1
    return seeded


def canonical_graph(
    scan: refs.RepoScan, index: refs.RefIndex, stoplist: frozenset[str], relation_weights: bool
) -> tuple[graph.RefGraph, int]:
    """Return Python's file graph with its edges in the canonical insertion order.

    The second value counts the statements that added two or more new edges, where Python's own
    insertion order depends on the hash seed.
    """
    built = graph.build_graph(scan, index, stoplist=stoplist, relation_weights=relation_weights)
    known = frozenset(built.nodes)
    paths = graph.PathIndex.build(known)
    order: EdgeOrder = {}
    seeded = 0
    for facts in scan.files:
        if refs.in_name_graph(facts):
            contributions = graph._reference_contributions(facts, index, stoplist, known, 1.0)
            order.update(dict.fromkeys((facts.path, target) for target, _ in contributions))
        seeded += import_keys(facts, paths, order)
        if relation_weights:
            inherited = graph._inheritance_targets(facts, index, known)
            order.update(dict.fromkeys((facts.path, target) for target in inherited))
    if set(order) != set(built.edges):
        sys.exit("refused: the replayed edge keys differ from Python's build_graph")
    edges = {key: built.edges[key] for key in order}
    return graph.RefGraph(nodes=built.nodes, edges=edges), seeded


def seed_sets(nodes: Sequence[str]) -> list[dict[str, float]]:
    if not nodes:
        return [{}]
    first, middle, last = nodes[0], nodes[len(nodes) // 2], nodes[-1]
    shares: dict[str, float] = {}
    for focus in ((first, middle), (last,), (middle, first, last)):
        for path in focus:
            shares[path] = shares.get(path, 0.0) + 1.0 / len(focus)
    return [{}, {first: 1.0}, shares, {first: 1.0, MISSING: 1.0}, {MISSING: 1.0}]


def pagerank_case(weighting: str, built: graph.RefGraph, seeds: dict[str, float]) -> Case:
    pure = sorted(path for path in built.nodes if is_test_path(path))
    ranking = graph.personalized_pagerank(built, seeds or None, pure_sources=set(pure))
    return {
        "weighting": weighting,
        "seeds": [[path, bits(share)] for path, share in seeds.items()],
        "pure_sources": pure,
        "converged": ranking.converged,
        "ranks": [[path, bits(rank)] for path, rank in ranking.rank.items()],
        "support": sorted(ranking.support),
        "order": graph.rank_order(ranking.rank, set(seeds)),
    }


def sample(items: Sequence[str], count: int) -> list[str]:
    """Return up to ``count`` items spread evenly over ``items``, in order."""
    if len(items) <= count:
        return list(items)
    step = (len(items) - 1) / (count - 1)
    return list(dict.fromkeys(items[round(position * step)] for position in range(count)))


def flood_case(built: graph.RefGraph, wanted: list[str]) -> Case:
    reach = graph.flood(built, frozenset(wanted), backward=True, max_depth=TEST_COMPANION_DEPTH)
    return {
        "seeds": wanted,
        "max_depth": TEST_COMPANION_DEPTH,
        "reached": [[row.path, row.depth] for row in reach.reached],
        "exhausted": reach.exhausted,
    }


def flood_seeds(nodes: Sequence[str]) -> list[list[str]]:
    singles = [[node] for node in sample(nodes, SAMPLED_NODES)]
    return [*singles, [path for path in nodes if not is_test_path(path)], [], [MISSING]]


def community_case(built: graph.RefGraph, resolution: float) -> Case:
    partition = communities.detect_communities(built, resolution=resolution)
    return {
        "resolution": bits(resolution),
        "communities": [
            [
                community.label,
                list(community.members),
                bits(community.internal_weight),
                bits(community.total_weight),
            ]
            for community in partition.communities
        ],
        "modularity": bits(partition.modularity),
        "standard_modularity": bits(partition.standard_modularity),
        "converged": partition.converged,
    }


def graph_case(resolved: resolve.ResolvedGraph) -> Case:
    arriving, leaving = resolved.ambiguous_degrees()
    return {
        "definitions": sorted(resolved.definitions),
        "files": list(resolved.files),
        "unresolved_imports": resolved.unresolved_imports,
        "unresolved_internal_imports": resolved.unresolved_internal_imports,
        "arriving": sorted([node, count] for node, count in arriving.items()),
        "leaving": sorted([node, count] for node, count in leaving.items()),
    }


def graph_nodes(resolved: resolve.ResolvedGraph) -> list[str]:
    return sorted(set(resolved.files) | set(resolved.definitions))


def adjacency_case(
    resolved: resolve.ResolvedGraph, node: str, position: EdgePosition, grouped: Grouped
) -> Case:
    """Return one node's grouped edges as positions in the edge list, and its ambiguous edges."""
    outgoing, incoming = grouped
    return {
        "node": node,
        "outgoing": [position[edge] for edge in outgoing.get(node, ())],
        "incoming": [position[edge] for edge in incoming.get(node, ())],
        "references_from": [reference_value(item) for item in resolved.references_from(node)],
        "candidates_at": [
            [name, endpoint_value(target)] for name, target in resolved.candidates_at(node)
        ],
        "ambiguous_from": [edge_value(edge) for edge in resolved.ambiguous_from(node)],
        "ambiguous_into": [edge_value(edge) for edge in resolved.ambiguous_into(node)],
    }


def adjacency_section(resolved: resolve.ResolvedGraph) -> list[object]:
    # Grouped once: each call to outgoing() or incoming() regroups the whole edge list.
    position = {edge: at for at, edge in enumerate(resolved.edges)}
    grouped = (resolved.outgoing(), resolved.incoming())
    return [adjacency_case(resolved, node, position, grouped) for node in graph_nodes(resolved)]


def path_case(
    resolved: resolve.ResolvedGraph, pair: tuple[str, str], policy: tuple[bool, bool], bound: int
) -> list[object]:
    """Return [source, target, unique, ambiguous, max_visited, found, visited, exhausted, hops]."""
    found = resolve.shortest_path(
        resolved,
        pair[0],
        pair[1],
        edge_policy=resolve.PathEdgePolicy(include_unique=policy[0], include_ambiguous=policy[1]),
        max_visited=bound,
    )
    hops = [[edge_value(hop.edge), hop.forward] for hop in found.hops]
    return [*pair, *policy, bound, found.found, found.visited, found.exhausted, hops]


def path_pairs(resolved: resolve.ResolvedGraph) -> list[tuple[str, str]]:
    """Return every ordered pair of distinct sampled nodes, then one node paired with itself."""
    chosen = sample(graph_nodes(resolved), SAMPLED_NODES)
    pairs = [(source, target) for source in chosen for target in chosen if source != target]
    return pairs + [(node, node) for node in chosen[:1]]


def paths_section(resolved: resolve.ResolvedGraph) -> list[object]:
    cases = [
        path_case(resolved, pair, policy, resolve.DEFAULT_MAX_VISITED)
        for policy in POLICIES
        for pair in path_pairs(resolved)
    ]
    reached = [
        (str(case[0]), str(case[1])) for case in cases if case[5] and case[3] and case[0] != case[1]
    ]
    for pair in reached[:BOUNDED_PAIRS]:
        cases.extend(path_case(resolved, pair, (True, True), bound) for bound in SMALL_BOUNDS)
    return list(cases)


def sites_case(facts: refs.FileFacts, resolver: resolve.Resolver) -> Case:
    position = {id(ref): number for number, ref in enumerate(facts.refs)}
    return {
        "path": facts.path,
        "sites": [
            [position[id(ref)], resolution.tier.value, ids(resolution.candidates)]
            for ref, resolution in resolve.resolved_sites(facts, resolver)
        ],
    }


def resolve_case(resolver: resolve.Resolver, path: str, name: str) -> list[object]:
    resolution = resolver.resolve(name, path)
    if resolution is None:
        return [path, name, None, []]
    return [path, name, resolution.tier.value, ids(resolution.candidates)]


def resolve_section(
    scan: refs.RepoScan, index: refs.RefIndex, resolver: resolve.Resolver
) -> list[object]:
    if len(scan.files) > RESOLVE_GRID_FILES:
        return []
    aliases = {name for scope in resolver.scopes.values() for name in scope.members}
    names = sorted(set(index.definitions) | aliases | {"nothing_defines_this"})
    return [resolve_case(resolver, facts.path, name) for facts in scan.files for name in names]


def through_section(resolver: resolve.Resolver) -> list[object]:
    return [
        [path, name, ids(resolver.through_imports(name, path))]
        for path, scope in sorted(resolver.scopes.items())
        for name in sorted(scope.members)
    ]


def definition_targets(definition: refs.Definition) -> set[str]:
    """Return the lookup spellings one definition answers to, and near misses of them."""
    stable = symbol_stable_id(definition.symbol)
    spelled = qualname(definition.symbol)
    numbered = id_qualname(definition.symbol)
    module = PurePosixPath(definition.path).with_suffix("").as_posix().replace("/", ".")
    tail = module.rpartition(".")[2]
    return {
        stable,
        stable.replace(definition.path, MISSING, 1),
        spelled,
        numbered,
        f"{module}.{spelled}",
        f"{module}.{numbered}",
        f"{tail}.{spelled}",
        f"elsewhere.{spelled}",
        f"{spelled}#2",
        f"{spelled}#1234567",
    }


def lookup_targets(index: refs.RefIndex) -> list[str]:
    targets = {"", ".", "a.", ".a", "py:", "py:x.py::", "::x", "nothing_defines_this", "f#2"}
    for definitions in index.definitions.values():
        for definition in definitions:
            targets |= definition_targets(definition)
    return sorted(targets)


def lookup_case(index: refs.RefIndex, target: str) -> list[object]:
    found = refs.resolve_definitions(index, target)
    return [target, ids(found.definitions), found.scoped]


def references_case(index: refs.RefIndex, definition: refs.Definition) -> list[object]:
    sites = refs.references_to(index, definition)
    return [
        symbol_stable_id(definition.symbol),
        [[site.path, site.line, site.role.value] for site in sites],
    ]


def owners_case(facts: refs.FileFacts) -> Case:
    def owner_id(line: int) -> str | None:
        symbol = refs.enclosing_symbol(facts, line)
        return None if symbol is None else symbol_stable_id(symbol)

    owners = refs.line_owners(facts)
    return {
        "path": facts.path,
        "enclosing": [owner_id(line) for line in range(facts.line_count + 2)],
        "owners": [[line, symbol_stable_id(owners[line])] for line in sorted(owners)],
    }


def name_sections(scan: refs.RepoScan, index: refs.RefIndex) -> Fixture:
    """Return the input facts and the name index."""
    return {
        "files": [file_case(facts) for facts in scan.files],
        "index": [
            name_case(index, name) for name in sorted(set(index.definitions) | set(index.sites))
        ],
    }


def lookup_sections(scan: refs.RepoScan, index: refs.RefIndex) -> Fixture:
    """Return the lookups, the reference sites of each definition, and the line owners."""
    definitions = [entry for name in sorted(index.definitions) for entry in index.definitions[name]]
    return {
        "lookups": [lookup_case(index, target) for target in lookup_targets(index)],
        "references": [references_case(index, definition) for definition in definitions],
        "owners": [owners_case(facts) for facts in scan.files],
    }


def file_graph_sections(scan: refs.RepoScan, index: refs.RefIndex) -> Fixture:
    """Return the file graph under each weighting, and the ranks, floods and communities."""
    graphs: dict[str, graph.RefGraph] = {}
    for weighting, (stoplist, relation_weights) in WEIGHTINGS.items():
        graphs[weighting], seeded = canonical_graph(scan, index, stoplist, relation_weights)
        if seeded:
            print(f"{scan.root.name}: {seeded} hash-ordered statements", file=sys.stderr)
    default = graphs["default"]
    return {
        "file_graph": [
            [weighting, source, target, bits(weight)]
            for weighting, built in graphs.items()
            for (source, target), weight in built.edges.items()
        ],
        "pagerank": [
            pagerank_case(weighting, built, seeds)
            for weighting, built in graphs.items()
            for seeds in seed_sets(built.nodes)
        ],
        "flood": [flood_case(default, wanted) for wanted in flood_seeds(default.nodes)],
        "communities": [community_case(default, resolution) for resolution in RESOLUTIONS],
    }


def symbol_sections(scan: refs.RepoScan, index: refs.RefIndex) -> Fixture:
    """Return the scopes, the symbol graph, its paths and cycles, and the resolver's answers."""
    resolver, resolved = resolve.resolve_repo(scan, index)
    imports = resolve.import_graph(scan.files)
    return {
        "scopes": [scope_case(path, scope) for path, scope in sorted(resolver.scopes.items())],
        "symbol_edges": [edge_value(edge) for edge in resolved.edges],
        "symbol_graph": [graph_case(resolved)],
        "ambiguous": [reference_value(reference) for reference in resolved.ambiguous],
        "candidates": [
            [name, [endpoint_value(target) for target in targets]]
            for name, targets in resolved.candidates.items()
        ],
        "adjacency": adjacency_section(resolved),
        "import_edges": [edge_value(edge) for edge in imports.edges],
        "import_graph": [graph_case(imports)],
        "cycles": [
            [list(cycle.files), list(cycle.members)] for cycle in resolve.import_cycles(imports)
        ],
        "paths": paths_section(resolved),
        "sites": [sites_case(facts, resolver) for facts in scan.files],
        "resolve": resolve_section(scan, index, resolver),
        "through_imports": through_section(resolver),
    }


def corpus(scan: refs.RepoScan, kept: Sequence[str] | None) -> Fixture:
    """Return the sections of one corpus file: every section, or those ``kept`` names."""
    index = refs.build_ref_index(scan)
    sections = {
        **name_sections(scan, index),
        **lookup_sections(scan, index),
        **file_graph_sections(scan, index),
        **symbol_sections(scan, index),
    }
    return sections if kept is None else {name: sections[name] for name in kept}


NORMPATH_INPUTS = (
    "",
    ".",
    "..",
    "/",
    "//",
    "///",
    "////a",
    "a/b/../c",
    "../a",
    "a/../../b",
    "//a/../b",
    "///a/./b",
    "a//b/./c/",
    "./",
    "a/..",
    "/..",
    "/../a",
    "src/a/../pricing",
    "a/./../b",
    "./a/../..",
    "../../x",
    "a/b/",
    "é/../ü",
)

STEM_INPUTS = (
    "a.py",
    "a.",
    "a.b.",
    ".bashrc",
    "x/.py",
    "a..b",
    "a/b.tar.gz",
    "dir.d/file",
    "...",
    "..",
    ".a.b",
    "//a/b.py",
    "a//b.py",
    "./a.py",
    "a/./b.py",
    "a/../b.py",
    "__init__.py",
    "src/pkg/__init__.py",
    "web/index.ts",
    "web/index.d.ts",
    "web/Index.ts",
    "pkg/__init__",
    "été/café.py",
)

TAIL_INPUTS = ("", ".", "a", "a.b", "a/b", "a..b", ".a", "a.", "pkg.mod", "é.ü", "a::b")

UNIT_PATHS = (
    ".py",
    "__init__.py",
    "a.b.c",
    "index.ts",
    "lib/common.sh",
    "main.py",
    "native/util.c",
    "native/util.h",
    "pkg/__init__.py",
    "pkg/core.py",
    "pkg/sub/__init__.py",
    "pkg/sub/deep.py",
    "scripts/lib/common.sh",
    "src/app/main.py",
    "src/app/util.py",
    "src/lib/__init__.py",
    "src/mycore/refs.py",
    "src/core/refs.py",
    "util.h",
    "web/shared/index.js",
    "web/src/index.ts",
    "web/src/util.ts",
    "x.py",
    "été/café.py",
)

# (importer, module, names, is_relative, relative_level): every candidate-base branch.
UNIT_IMPORTS: tuple[tuple[str, str, tuple[str, ...], bool, int], ...] = (
    ("main.py", "pkg.core", (), False, 0),
    ("main.py", "pkg", ("core", "missing", "sub"), False, 0),
    ("main.py", "  pkg.core  ", (), False, 0),
    ("main.py", "", (), False, 0),
    ("main.py", "", ("x",), True, 1),
    ("main.py", "app.util", (), False, 0),
    ("main.py", "core.refs", (), False, 0),
    ("main.py", "lib", (), False, 0),
    ("main.py", "src.lib", ("x",), False, 0),
    ("main.py", "a.b.c", (), False, 0),
    ("main.py", "/abs/x", (), False, 0),
    ("main.py", "été.café", (), False, 0),
    ("x.py", "", (), True, 2),
    ("pkg/sub/deep.py", "", ("helpers", "core"), True, 2),
    ("pkg/sub/deep.py", "core", ("run",), True, 2),
    ("pkg/sub/deep.py", "pkg", (), True, 3),
    ("pkg/sub/deep.py", "far", (), True, 4),
    ("pkg/sub/deep.py", "", (), True, 1),
    ("web/src/index.ts", "./util", ("a",), False, 0),
    ("web/src/index.ts", "../shared", (), False, 0),
    ("web/src/index.ts", "./lib/../util", (), False, 0),
    ("web/src/index.ts", "../../../../up", (), False, 0),
    ("web/src/index.ts", ".", (), False, 0),
    ("native/main.c", "util.h", (), False, 0),
    ("main.c", "util.h", (), False, 0),
    ("scripts/run.sh", "lib/common.sh", (), False, 0),
    ("deep/x.py", "//a", (), False, 0),
    ("src/app/main.py", "util", (), True, 0),
)

NAMES = ("", "a", "ab", "abc", "é", "éé", "ééé", "ctx", "x\U0001d538")
SPREADS = (0, 1, 2, 3, 10, 100, 12345, 2**31)

LABEL_MEMBERS: tuple[tuple[str, ...], ...] = (
    (),
    ("a.py",),
    ("a.py", "b.py"),
    ("src/a.py", "src/b.py"),
    ("src/a.py", "lib/b.py"),
    ("src/core/a.py", "src/core/b.py", "src/util/c.py"),
    ("src/core/a.py", "src/util/b.py", "lib/c.py", "lib/d.py"),
    ("a/b/c/d.py", "a/b/c/e.py", "a/b/f.py", "a/g.py", "h.py"),
)

SEGMENT_PATHS = ("a.tar.gz", ".bashrc", "src/agentless_mcp/core/refs.py", "pkg/mod", "x/y.z/w.py")
SEGMENT_MODULES = (
    "",
    "   ",
    "json",
    "src.x",
    "agentless_mcp.core",
    "a.tar",
    ".bashrc",
    "bashrc",
    "pkg\\mod",
    "pkg::mod",
    "y.z",
    "x/y",
    "./x",
    " refs ",
)

FLOOD_LEAVES = 20_001
FLOOD_OUTERS = 10


def normpath_case(text: str) -> list[object]:
    return [text, posixpath.normpath(text), graph._normalized(PurePosixPath(text))]


def stem_case(path: str) -> list[object]:
    return [path, graph._stem(path), list(graph._module_stems(path))]


def import_case(importer: str, statement: ImportStatement, paths: graph.PathIndex) -> Case:
    return {
        "importer": importer,
        "statement": import_value(statement),
        "target": graph.resolve_import_target(importer, statement, paths),
        "submodules": [
            [name, graph.resolve_imported_submodule(importer, statement, name, paths)]
            for name in statement.names
        ],
    }


def import_cases() -> list[object]:
    paths = graph.PathIndex.build(UNIT_PATHS)
    cases: list[object] = []
    for importer, module, names, relative, level in UNIT_IMPORTS:
        statement = ImportStatement(
            module=module, names=names, is_relative=relative, relative_level=level, line_number=1
        )
        cases.append(import_case(importer, statement, paths))
    return cases


def flood_bound_case() -> Case:
    """Return a backward flood that passes the visit bound, over a star the Rust test rebuilds."""
    leaves = [f"leaf{number:05d}.py" for number in range(FLOOD_LEAVES)]
    outers = [f"outer{number:02d}.py" for number in range(FLOOD_OUTERS)]
    edges = dict.fromkeys(((leaf, "hub.py") for leaf in leaves), 1.0)
    edges.update(dict.fromkeys(zip(outers, leaves, strict=False), 1.0))
    built = graph.RefGraph(nodes=tuple(sorted(["hub.py", *leaves, *outers])), edges=edges)
    reach = graph.flood(built, {"hub.py"}, backward=True, max_depth=TEST_COMPANION_DEPTH)
    rows = [[row.path, row.depth] for row in reach.reached]
    return {
        "leaves": FLOOD_LEAVES,
        "outers": FLOOD_OUTERS,
        "max_depth": TEST_COMPANION_DEPTH,
        "rows": len(rows),
        "first": rows[:3],
        "last": rows[-12:],
        "exhausted": reach.exhausted,
    }


def units() -> Fixture:
    """Return the rule cases over chosen inputs."""
    segments = resolve._repository_segments(SEGMENT_PATHS)
    stoplists: tuple[frozenset[str], ...] = (frozenset(), frozenset({"ctx", "abc"}))
    return {
        "normpath": [normpath_case(text) for text in NORMPATH_INPUTS],
        "stems": [stem_case(path) for path in STEM_INPUTS + UNIT_PATHS],
        "module_tails": [[module, graph._module_tail(module)] for module in TAIL_INPUTS],
        "unit_paths": [list(UNIT_PATHS)],
        "weightings": [
            [name, sorted(stoplist), relation_weights]
            for name, (stoplist, relation_weights) in WEIGHTINGS.items()
        ],
        "suffix_match": [
            [module, graph.PathIndex.build(UNIT_PATHS).suffix_match(module)]
            for module in (
                *TAIL_INPUTS,
                "core.refs",
                "app.util",
                "src.lib",
                "lib.common",
                "web.src",
            )
        ],
        "imports": import_cases(),
        "name_multiplier": [
            [name, sorted(stoplist), bits(graph.name_multiplier(name, stoplist))]
            for name in NAMES
            for stoplist in stoplists
        ],
        "damping": [[spread, bits(graph.common_name_damping(spread))] for spread in SPREADS],
        "community_label": [
            [list(members), communities.community_label(members)] for members in LABEL_MEMBERS
        ],
        "segments": [[list(SEGMENT_PATHS), sorted(segments)]],
        "names_this_repository": [
            [module, resolve._names_this_repository(module, segments)] for module in SEGMENT_MODULES
        ],
        "flood_bound": [flood_bound_case()],
    }


def scan_sources(name: str, sources: Sources, extractor: TreeSitterExtractor) -> refs.RepoScan:
    """Write ``sources`` to a temporary repository named ``name`` and scan it."""
    with tempfile.TemporaryDirectory(prefix="graph-fixture-") as made:
        root = Path(made) / name
        for path, text in sources.items():
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text, encoding="utf-8")
        return refs.scan_repo(root, extractor)


def scans() -> dict[str, refs.RepoScan]:
    extractor = TreeSitterExtractor()
    found = {name: refs.scan_repo(CORPUS / name, extractor) for name in REPOSITORIES}
    found[TREE] = refs.scan_repo(CORPUS, extractor)
    for name, sources in EXTENSIONS.items():
        found[name] = scan_sources(name, sources, extractor)
    skipped = [f"{name}: {file.path}" for name, scan in found.items() for file in scan.skipped]
    if skipped:
        sys.exit(f"refused: the scan skipped files: {', '.join(skipped)}")
    return found


def build() -> dict[Path, bytes]:
    files = {
        FIXTURES / "corpora" / f"{name}.json": render(corpus(scan, TREE_SECTIONS.get(name)))
        for name, scan in scans().items()
    }
    files[FIXTURES / "units.json"] = render(units())
    return files


def main() -> int:
    refuse_real_caches()
    if not os.environ.get(GRAMMAR_OVERRIDE):
        sys.exit(f"refused: set {GRAMMAR_OVERRIDE} to a scratch copy of the grammar libraries")
    os.environ["AGENTLESS_MCP_NO_DOWNLOAD"] = "1"
    os.environ["AGENTLESS_MCP_NO_AUTO_WARM"] = "1"
    description = (__doc__ or "").splitlines()[0]
    return write_or_check_files(description, FIXTURES / "corpora", build)


if __name__ == "__main__":
    raise SystemExit(main())
