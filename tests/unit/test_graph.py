"""The reference graph and the personalized PageRank over it."""

import math

import pytest

from agentless_mcp.core.graph import (
    AMBIGUOUS_MATCH_MULTIPLIER,
    IMPORT_EDGE_WEIGHT,
    NOISE_NAME_MULTIPLIER,
    UNIQUE_MATCH_MULTIPLIER,
    RefGraph,
    build_graph,
    name_multiplier,
    personalized_pagerank,
    rank_order,
    resolve_import_target,
)
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.refs import build_ref_index, scan_repo

# A package tree with the two relative-import forms, laid out the way a Python
# package normally is: the importer sits one directory below the package root
# and reaches sideways and upward.
PACKAGE = {
    "pkg/__init__.py": "",
    "pkg/helper.py": "def thing():\n    return 1\n",
    "pkg/sub/__init__.py": "",
    "pkg/sub/sibling.py": "def near():\n    return 2\n",
    "pkg/sub/user.py": (
        "from . import sibling\n"
        "from ..helper import thing\n"
        "\n"
        "\n"
        "def go():\n"
        "    return sibling.near() + thing()\n"
    ),
}


def line(*paths_and_weights):
    """Build an edge map from ``(source, target, weight)`` triples."""
    return {(source, target): weight for source, target, weight in paths_and_weights}


def write(root, files):
    """Write a mapping of relative path to text under ``root``."""
    for relative, text in files.items():
        (root / relative).parent.mkdir(parents=True, exist_ok=True)
        (root / relative).write_text(text, encoding="utf-8")
    return root


def graph_of(root, extractor):
    """Scan a repository and build its reference graph."""
    scan = scan_repo(root, extractor)
    return build_graph(scan, build_ref_index(scan))


def js_import(module):
    """A JavaScript-style relative import statement of ``module``."""
    return ImportStatement(
        module=module,
        names=(),
        is_relative=True,
        relative_level=0,
        line_number=1,
        resolved_path="",
    )


class TestPageRank:
    def test_an_empty_graph_ranks_nothing(self):
        assert personalized_pagerank(RefGraph(nodes=(), edges={})) == {}

    def test_ranks_sum_to_one(self):
        graph = RefGraph(
            nodes=("a.py", "b.py", "c.py"),
            edges=line(("a.py", "b.py", 1.0), ("c.py", "b.py", 1.0)),
        )
        rank = personalized_pagerank(graph)
        assert math.isclose(sum(rank.values()), 1.0, rel_tol=1e-6)

    def test_the_referenced_file_outranks_its_referrers(self):
        graph = RefGraph(
            nodes=("a.py", "b.py", "c.py"),
            edges=line(("a.py", "b.py", 1.0), ("c.py", "b.py", 1.0)),
        )
        assert rank_order(personalized_pagerank(graph))[0] == "b.py"

    def test_two_independent_builds_of_one_tree_rank_identically(self, tmp_path, extractor):
        """The determinism claim is about the pipeline, not about a pure call.

        Two scans of the same tree are two independently-constructed inputs:
        separate walks, separate parses, separate edge dictionaries. The
        rankings they produce must agree bit for bit.
        """
        root = write(tmp_path, PACKAGE)
        first = graph_of(root, extractor)
        second = graph_of(root, extractor)

        assert first.nodes == second.nodes
        assert first.edges == second.edges
        assert personalized_pagerank(first) == personalized_pagerank(second)

    def test_ties_are_broken_by_path_order(self):
        graph = RefGraph(nodes=("b.py", "a.py", "c.py"), edges={})
        assert rank_order(personalized_pagerank(graph)) == ["a.py", "b.py", "c.py"]

    def test_seeds_take_the_teleport_mass(self):
        graph = RefGraph(
            nodes=("a.py", "b.py", "c.py"),
            edges=line(("a.py", "b.py", 1.0), ("c.py", "b.py", 1.0)),
        )
        unfocused = personalized_pagerank(graph)
        focused = personalized_pagerank(graph, {"c.py": 1.0})

        assert focused["c.py"] > unfocused["c.py"]
        assert focused["a.py"] < unfocused["a.py"]

    def test_a_seed_outside_the_graph_falls_back_to_uniform(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={})
        assert personalized_pagerank(graph, {"gone.py": 1.0}) == personalized_pagerank(graph)

    def test_damping_zero_is_the_personalization_vector(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges=line(("a.py", "b.py", 1.0)))
        rank = personalized_pagerank(graph, {"a.py": 1.0}, damping=0.0)
        assert math.isclose(rank["a.py"], 1.0, rel_tol=1e-6)
        assert math.isclose(rank["b.py"], 0.0, abs_tol=1e-6)

    def test_dangling_mass_goes_to_the_seeds_not_uniformly(self):
        """b.py references nothing, so its mass must return to the seed."""
        graph = RefGraph(nodes=("a.py", "b.py", "c.py"), edges=line(("a.py", "b.py", 1.0)))
        rank = personalized_pagerank(graph, {"a.py": 1.0})
        assert rank["a.py"] > rank["b.py"] > rank["c.py"]

    def test_the_default_damping_is_the_one_the_rankings_were_tuned_against(self):
        """Pin what 0.85 *does*, so changing it fails here rather than silently.

        ``a.py`` references ``b.py`` and nothing references back, so the fixed
        point is closed-form: with a uniform teleport vector and ``b.py``'s
        dangling mass returned to it, ``a.py`` settles at
        ``0.5 / (1 + damping / 2)``, which is 0.350877 at 0.85 and moves for
        any other damping.
        """
        graph = RefGraph(nodes=("a.py", "b.py"), edges=line(("a.py", "b.py", 1.0)))
        rank = personalized_pagerank(graph)

        assert math.isclose(rank["a.py"], 0.3508771, abs_tol=1e-6)
        assert math.isclose(rank["b.py"], 0.6491228, abs_tol=1e-6)


class TestNameWeighting:
    def test_short_names_are_damped(self):
        assert name_multiplier("i") == NOISE_NAME_MULTIPLIER
        assert name_multiplier("ok") == NOISE_NAME_MULTIPLIER
        assert name_multiplier("quote") == 1.0

    def test_a_widespread_name_contributes_less_than_a_rare_one(self, tmp_path, extractor):
        """Log damping, measured on two files defining one name each.

        ``rare`` is referenced from one file, ``common`` from five. The edge a
        single reference to ``common`` buys must be worth less.
        """
        (tmp_path / "rare_def.py").write_text(
            "def rare_helper():\n    return 1\n", encoding="utf-8"
        )
        (tmp_path / "common_def.py").write_text(
            "def common_helper():\n    return 2\n", encoding="utf-8"
        )
        (tmp_path / "caller.py").write_text(
            "def go():\n    return rare_helper() + common_helper()\n", encoding="utf-8"
        )
        for number in range(4):
            (tmp_path / f"extra{number}.py").write_text(
                "def use():\n    return common_helper()\n", encoding="utf-8"
            )

        scan = scan_repo(tmp_path, extractor)
        graph = build_graph(scan, build_ref_index(scan))

        rare_edge = graph.edges[("caller.py", "rare_def.py")]
        common_edge = graph.edges[("caller.py", "common_def.py")]
        assert rare_edge > common_edge


class TestBuildGraph:
    def test_imports_add_a_weighted_edge(self, tmp_path, extractor):
        (tmp_path / "leaf.py").write_text("VALUE = 1\n", encoding="utf-8")
        (tmp_path / "user.py").write_text("import leaf\n", encoding="utf-8")

        scan = scan_repo(tmp_path, extractor)
        graph = build_graph(scan, build_ref_index(scan))

        assert graph.edges[("user.py", "leaf.py")] >= IMPORT_EDGE_WEIGHT

    def test_a_file_never_points_at_itself(self, tmp_path, extractor):
        (tmp_path / "solo.py").write_text(
            "def helper():\n    return 1\n\n\ndef go():\n    return helper()\n", encoding="utf-8"
        )
        scan = scan_repo(tmp_path, extractor)
        graph = build_graph(scan, build_ref_index(scan))
        assert ("solo.py", "solo.py") not in graph.edges

    def test_name_only_edges_are_discounted_by_resolution_tier(self, tmp_path, extractor):
        (tmp_path / "unique.py").write_text("def solitary():\n    return 1\n", encoding="utf-8")
        (tmp_path / "left.py").write_text("def crowded():\n    return 1\n", encoding="utf-8")
        (tmp_path / "right.py").write_text("def crowded():\n    return 2\n", encoding="utf-8")
        (tmp_path / "consumer.py").write_text(
            "def use():\n    return solitary() + crowded()\n",
            encoding="utf-8",
        )

        graph = graph_of(tmp_path, extractor)
        unique = graph.edges[("consumer.py", "unique.py")]
        ambiguous = graph.edges[("consumer.py", "left.py")]

        expected_ratio = UNIQUE_MATCH_MULTIPLIER / AMBIGUOUS_MATCH_MULTIPLIER
        assert math.isclose(unique / ambiguous, expected_ratio)

    def test_same_file_definition_shadows_name_matches_in_other_files(self, tmp_path, extractor):
        (tmp_path / "local.py").write_text(
            "def helper():\n    return 1\n\n\ndef use():\n    return helper()\n",
            encoding="utf-8",
        )
        (tmp_path / "other.py").write_text("def helper():\n    return 2\n", encoding="utf-8")

        graph = graph_of(tmp_path, extractor)

        assert ("local.py", "other.py") not in graph.edges

    def test_a_locally_bound_name_buys_no_edge_to_an_unrelated_file(self, tmp_path, extractor):
        """A parameter spells its own binding, not a symbol in another file.

        ``consumer.py`` never mentions ``library.py`` -- it happens to name a
        parameter the way another file names a function -- so the map must not
        report a relationship between them.
        """
        (tmp_path / "library.py").write_text("def payload():\n    return 1\n", encoding="utf-8")
        (tmp_path / "consumer.py").write_text(
            "def go(payload):\n    return payload\n", encoding="utf-8"
        )

        graph = graph_of(tmp_path, extractor)

        assert ("consumer.py", "library.py") not in graph.edges

    def test_a_local_assignment_does_not_create_a_unique_edge(self, tmp_path, extractor):
        (tmp_path / "library.py").write_text("def counter():\n    return 1\n", encoding="utf-8")
        (tmp_path / "consumer.py").write_text(
            "def go():\n    counter = 0\n    return counter\n",
            encoding="utf-8",
        )

        graph = graph_of(tmp_path, extractor)

        assert ("consumer.py", "library.py") not in graph.edges

    def test_keyword_and_attribute_names_buy_no_bare_edges(self, tmp_path, extractor):
        (tmp_path / "library.py").write_text(
            "def graphs():\n    return 1\n\n\ndef write():\n    return 2\n",
            encoding="utf-8",
        )
        (tmp_path / "consumer.py").write_text(
            "def go(call, stream, value):\n    return call(graphs=value, stderr=stream.write)\n",
            encoding="utf-8",
        )

        graph = graph_of(tmp_path, extractor)

        assert ("consumer.py", "library.py") not in graph.edges


class TestGraphInvariants:
    """An edge naming a node outside the graph is refused the same way, both ends."""

    def test_an_edge_to_an_unknown_node_is_refused_at_construction(self):
        with pytest.raises(ValueError, match=r"ghost\.py"):
            RefGraph(nodes=("a.py",), edges=line(("a.py", "ghost.py", 1.0)))

    def test_an_edge_from_an_unknown_node_is_refused_at_construction(self):
        with pytest.raises(ValueError, match=r"ghost\.py"):
            RefGraph(nodes=("a.py",), edges=line(("ghost.py", "a.py", 1.0)))

    def test_a_repeated_node_is_refused_at_construction(self):
        with pytest.raises(ValueError, match=r"a\.py"):
            RefGraph(nodes=("a.py", "a.py"), edges={})


class TestRelativeImports:
    def test_a_bare_relative_import_resolves_the_package_it_names(self, tmp_path, extractor):
        """``from . import sibling`` declares a dependency on the package itself."""
        graph = graph_of(write(tmp_path, PACKAGE), extractor)

        assert graph.edges[("pkg/sub/user.py", "pkg/sub/__init__.py")] >= IMPORT_EDGE_WEIGHT

    def test_a_parent_relative_import_resolves_upward(self, tmp_path, extractor):
        """``from ..helper import thing`` is a declared edge, not a name collision."""
        graph = graph_of(write(tmp_path, PACKAGE), extractor)

        assert graph.edges[("pkg/sub/user.py", "pkg/helper.py")] >= IMPORT_EDGE_WEIGHT

    def test_a_parent_relative_javascript_import_resolves_upward(self):
        assert (
            resolve_import_target("src/a/b.ts", js_import("../pricing"), ["src/pricing.ts"])
            == "src/pricing.ts"
        )

    def test_a_sibling_relative_javascript_import_still_resolves(self):
        assert (
            resolve_import_target("src/a/b.ts", js_import("./pricing"), ["src/a/pricing.ts"])
            == "src/a/pricing.ts"
        )


class TestImportResolution:
    def test_a_relative_javascript_import_resolves_to_a_sibling(self):
        statement = ImportStatement(
            module="./pricing",
            names=(),
            is_relative=True,
            relative_level=0,
            line_number=1,
            resolved_path="",
        )
        assert (
            resolve_import_target("src/inventory.ts", statement, ["src/pricing.ts"])
            == "src/pricing.ts"
        )

    def test_a_dotted_python_module_resolves_through_a_src_layout(self):
        statement = ImportStatement(
            module="agentless_mcp.core.refs",
            names=(),
            is_relative=False,
            relative_level=0,
            line_number=1,
            resolved_path="",
        )
        assert (
            resolve_import_target(
                "src/agentless_mcp/application/map_service.py",
                statement,
                ["src/agentless_mcp/core/refs.py", "docs/refs.py"],
            )
            == "src/agentless_mcp/core/refs.py"
        )

    def test_an_unresolvable_import_is_none_not_a_guess(self):
        statement = ImportStatement(
            module="requests",
            names=(),
            is_relative=False,
            relative_level=0,
            line_number=1,
            resolved_path="",
        )
        assert resolve_import_target("app.py", statement, ["app.py", "other.py"]) is None


class TestSuffixMatching:
    """A dotted module matches the tail of a path, on component boundaries."""

    @staticmethod
    def absolute(module):
        return ImportStatement(
            module=module,
            names=(),
            is_relative=False,
            relative_level=0,
            line_number=1,
            resolved_path="",
        )

    def test_a_src_layout_resolves_through_the_tail(self):
        known = ["src/agentless_mcp/core/refs.py", "app.py"]
        target = resolve_import_target("app.py", self.absolute("agentless_mcp.core.refs"), known)
        assert target == "src/agentless_mcp/core/refs.py"

    def test_the_tail_must_land_on_a_component_boundary(self):
        # "src/mycore/refs" ends with the string "core/refs" but names a
        # different package. It is also the shorter path, so the tie-break
        # actively preferred it over the file the import names.
        known = ["src/mycore/refs.py", "src/agentless_mcp/core/refs.py"]
        target = resolve_import_target("app.py", self.absolute("agentless_mcp.core.refs"), known)
        assert target == "src/agentless_mcp/core/refs.py"

    def test_a_module_matching_nothing_whole_resolves_to_nothing(self):
        known = ["src/mycore/refs.py"]
        assert resolve_import_target("app.py", self.absolute("core.refs"), known) is None


class TestASpecifierThatAlreadyNamesAFile:
    """C, C++ and shell write the filename; Python writes a dotted module.

    Before this, every specifier was dotted the way a Python module string is,
    so `#include "money.h"` became the stem `money/h` and no suffix appended
    to that ever matched. Measured: no C or C++ include resolved to a
    repository file at all, on any repository -- the include graph was empty,
    and the audit's finding about *which* file such an include preferred could
    not arise because it never reached one.
    """

    KNOWN = frozenset({"money.h", "app.c", "src/a/util.h", "src/a/app.c", "util.h", "lib/util.sh"})

    def statement(self, module: str, *, relative: bool = True) -> ImportStatement:
        return ImportStatement(
            module=module,
            names=(),
            is_relative=relative,
            relative_level=0,
            line_number=1,
            resolved_path="",
        )

    def test_a_quoted_include_resolves_to_the_file_it_names(self):
        assert resolve_import_target("app.c", self.statement("money.h"), self.KNOWN) == "money.h"

    def test_it_prefers_the_sibling_over_a_same_named_file_at_the_root(self):
        # `#include "util.h"` names the header beside the including file.
        # Both exist here, so the preference is the whole assertion.
        assert (
            resolve_import_target("src/a/app.c", self.statement("util.h"), self.KNOWN)
            == "src/a/util.h"
        )

    def test_a_system_header_still_resolves_to_nothing(self):
        # `<stdio.h>` is not in this repository, and inventing a match for it
        # would be the guess this resolver refuses to make.
        assert resolve_import_target("app.c", self.statement("stdio.h"), self.KNOWN) is None

    def test_a_shell_source_path_resolves_too(self):
        assert (
            resolve_import_target("app.c", self.statement("lib/util.sh"), self.KNOWN)
            == "lib/util.sh"
        )

    def test_a_dotted_python_module_is_unaffected(self):
        # The branch keys on "the specifier already ends in a file suffix",
        # so `package.module` still goes through the dotted path.
        known = frozenset({"package/module.py", "app.py"})
        assert (
            resolve_import_target("app.py", self.statement("package.module", relative=False), known)
            == "package/module.py"
        )
