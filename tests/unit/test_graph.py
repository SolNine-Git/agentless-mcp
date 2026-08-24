"""The reference graph and the personalized PageRank over it."""

import math
from dataclasses import replace
from types import MappingProxyType

import pytest

from agentless_mcp.core.graph import (
    AMBIGUOUS_MATCH_MULTIPLIER,
    DEFAULT_MAX_ITERATIONS,
    IMPORT_EDGE_WEIGHT,
    NOISE_NAME_MULTIPLIER,
    UNIQUE_MATCH_MULTIPLIER,
    PathIndex,
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


class TestGraphIsAValue:
    def test_the_caller_cannot_mutate_a_built_graph(self):
        """A validating constructor promises the value cannot become invalid.

        The edge map used to be stored by reference, so one write through the
        mapping the caller still held added an edge naming a node
        ``__post_init__`` had just refused -- and the ranking met it as a
        ``KeyError`` from inside the numeric loop.
        """
        edges = {("a.py", "b.py"): 1.0}
        graph = RefGraph(nodes=("a.py", "b.py"), edges=edges)

        edges[("a.py", "vendor/gone.py")] = 99.0

        assert ("a.py", "vendor/gone.py") not in graph.edges
        assert personalized_pagerank(graph)

    def test_the_mapping_the_graph_hands_out_is_read_only(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={("a.py", "b.py"): 1.0})

        assert isinstance(graph.edges, MappingProxyType)


class TestPageRank:
    def test_an_empty_graph_ranks_nothing(self):
        assert personalized_pagerank(RefGraph(nodes=(), edges={})).rank == {}

    def test_ranks_sum_to_one(self):
        graph = RefGraph(
            nodes=("a.py", "b.py", "c.py"),
            edges=line(("a.py", "b.py", 1.0), ("c.py", "b.py", 1.0)),
        )
        rank = personalized_pagerank(graph).rank
        assert math.isclose(sum(rank.values()), 1.0, rel_tol=1e-6)

    def test_the_referenced_file_outranks_its_referrers(self):
        graph = RefGraph(
            nodes=("a.py", "b.py", "c.py"),
            edges=line(("a.py", "b.py", 1.0), ("c.py", "b.py", 1.0)),
        )
        assert rank_order(personalized_pagerank(graph).rank)[0] == "b.py"

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
        assert personalized_pagerank(first).rank == personalized_pagerank(second).rank

    def test_ties_are_broken_by_path_order(self):
        graph = RefGraph(nodes=("b.py", "a.py", "c.py"), edges={})
        assert rank_order(personalized_pagerank(graph).rank) == ["a.py", "b.py", "c.py"]

    def test_seeds_take_the_teleport_mass(self):
        graph = RefGraph(
            nodes=("a.py", "b.py", "c.py"),
            edges=line(("a.py", "b.py", 1.0), ("c.py", "b.py", 1.0)),
        )
        unfocused = personalized_pagerank(graph).rank
        focused = personalized_pagerank(graph, {"c.py": 1.0}).rank

        assert focused["c.py"] > unfocused["c.py"]
        assert focused["a.py"] < unfocused["a.py"]

    def test_a_seed_outside_the_graph_falls_back_to_uniform(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges={})
        assert (
            personalized_pagerank(graph, {"gone.py": 1.0}).rank == personalized_pagerank(graph).rank
        )

    def test_a_negative_seed_weight_is_refused_rather_than_clamped(self):
        """Teleport mass has no negative direction, so the number is a mistake.

        Clamped to zero it read as "this file was not seeded", which is the
        one thing it is not: the caller named the file and asked for it.
        """
        graph = RefGraph(nodes=("a.py", "b.py"), edges={})

        with pytest.raises(ValueError, match=r"a\.py"):
            personalized_pagerank(graph, {"a.py": -1.0, "b.py": 1.0})

    def test_a_negative_weight_on_an_unlisted_file_is_refused_too(self):
        graph = RefGraph(nodes=("a.py",), edges={})

        with pytest.raises(ValueError, match=r"gone\.py"):
            personalized_pagerank(graph, {"gone.py": -1.0})

    def test_damping_zero_is_the_personalization_vector(self):
        graph = RefGraph(nodes=("a.py", "b.py"), edges=line(("a.py", "b.py", 1.0)))
        rank = personalized_pagerank(graph, {"a.py": 1.0}, damping=0.0).rank
        assert math.isclose(rank["a.py"], 1.0, rel_tol=1e-6)
        assert math.isclose(rank["b.py"], 0.0, abs_tol=1e-6)

    def test_dangling_mass_goes_to_the_seeds_not_uniformly(self):
        """b.py references nothing, so its mass must return to the seed."""
        graph = RefGraph(nodes=("a.py", "b.py", "c.py"), edges=line(("a.py", "b.py", 1.0)))
        rank = personalized_pagerank(graph, {"a.py": 1.0}).rank
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
        rank = personalized_pagerank(graph).rank

        assert math.isclose(rank["a.py"], 0.3508771, abs_tol=1e-6)
        assert math.isclose(rank["b.py"], 0.6491228, abs_tol=1e-6)


class TestConvergenceIsReported:
    """A ranking that ran out of passes must not read as a settled one."""

    def test_a_settled_run_says_so_and_names_its_pass_count(self):
        graph = RefGraph(
            nodes=("a.py", "b.py", "c.py"),
            edges=line(("a.py", "b.py", 1.0), ("c.py", "b.py", 1.0)),
        )

        ranking = personalized_pagerank(graph)

        assert ranking.converged
        assert 0 < ranking.iterations < DEFAULT_MAX_ITERATIONS

    def test_an_empty_graph_has_converged_over_nothing(self):
        ranking = personalized_pagerank(RefGraph(nodes=(), edges={}))

        assert ranking.rank == {}
        assert ranking.converged
        assert ranking.iterations == 0

    def test_a_run_that_spends_its_iterations_reports_an_unsettled_rank(self):
        """The bound is reachable at the shipped defaults, not only at 1 pass.

        A chain mixes slowly, and the damping is what decides how slowly.
        Measured 2026-08-23: this graph needs 191 passes at damping 0.99 to
        move less than the default epsilon, against a bound of 100. Nothing
        in the returned vector says which of the two happened, which is why
        the flag is on the value rather than left to the caller to infer from
        the pass count.
        """
        nodes = tuple(f"f{index:03d}.py" for index in range(40))
        edges = {(nodes[index], nodes[index + 1]): 1.0 for index in range(len(nodes) - 1)}
        graph = RefGraph(nodes=nodes, edges=edges)

        ranking = personalized_pagerank(graph, damping=0.99)

        assert not ranking.converged
        assert ranking.iterations == DEFAULT_MAX_ITERATIONS

    def test_the_same_graph_settles_once_it_is_given_the_passes(self):
        nodes = tuple(f"f{index:03d}.py" for index in range(40))
        edges = {(nodes[index], nodes[index + 1]): 1.0 for index in range(len(nodes) - 1)}
        graph = RefGraph(nodes=nodes, edges=edges)

        ranking = personalized_pagerank(graph, damping=0.99, max_iterations=500)

        assert ranking.converged
        assert ranking.iterations == 191


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


class TestPathIndex:
    """The tail table answers what the scan answered, at one lookup a name."""

    KNOWN = (
        "src/agentless_mcp/core/refs.py",
        "src/mycore/refs.py",
        "docs/refs.py",
        "app.py",
    )

    def test_the_table_and_the_scan_agree_on_every_module_tail(self):
        index = PathIndex.build(self.KNOWN)
        statement = ImportStatement(
            module="",
            names=(),
            is_relative=False,
            relative_level=0,
            line_number=1,
            resolved_path="",
        )
        modules = (
            "agentless_mcp.core.refs",
            "core.refs",
            "mycore.refs",
            "refs",
            "docs/refs",
            "nothing.at.all",
        )
        for module in modules:
            probe = replace(statement, module=module)
            scanned = resolve_import_target("app.py", probe, list(self.KNOWN))
            tabled = resolve_import_target("app.py", probe, index)
            assert tabled == scanned, module

    def test_the_shortest_match_still_wins_a_tie(self):
        """Two paths end on the same tail; the table stores the same winner."""
        index = PathIndex.build(("vendored/deep/core/refs.py", "x/core/refs.py"))

        assert index.suffix_match("core.refs") == "x/core/refs.py"

    def test_a_module_with_no_separator_never_matches(self):
        """A bare name has no tail to land on a boundary, so it names nothing."""
        assert PathIndex.build(self.KNOWN).suffix_match("refs") is None

    def test_the_index_answers_membership_for_the_direct_candidates(self):
        index = PathIndex.build(self.KNOWN)

        assert "app.py" in index
        assert "gone.py" not in index

    def test_a_build_resolves_the_same_edges_through_the_index(self, tmp_path, extractor):
        """``build_graph`` hands the index over; the edges must not move."""
        graph = graph_of(write(tmp_path, PACKAGE), extractor)

        assert graph.edges[("pkg/sub/user.py", "pkg/helper.py")] >= IMPORT_EDGE_WEIGHT


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


class TestARelativeLevelAboveTheRoot:
    """A level deeper than the importing file's directory names no file.

    ``PurePosixPath(".").parent`` is ``"."``, so walking up used to saturate
    at the repository root instead of running out -- and ``from ...... import
    x`` in a top-level module resolved exactly as confidently as ``from .
    import x``.
    """

    KNOWN = frozenset({"mod.py", "helper.py", "pkg/sub/user.py", "pkg/helper.py"})

    def leveled(self, level: int) -> ImportStatement:
        return ImportStatement(
            module="helper",
            names=(),
            is_relative=True,
            relative_level=level,
            line_number=1,
            resolved_path="",
        )

    def test_a_top_level_module_still_reaches_its_own_directory(self):
        assert resolve_import_target("mod.py", self.leveled(1), self.KNOWN) == "helper.py"

    def test_a_top_level_module_cannot_reach_above_the_repository(self):
        assert resolve_import_target("mod.py", self.leveled(2), self.KNOWN) is None

    def test_a_nested_module_reaches_the_root_but_no_further(self):
        assert resolve_import_target("pkg/sub/user.py", self.leveled(2), self.KNOWN) == (
            "pkg/helper.py"
        )
        assert resolve_import_target("pkg/sub/user.py", self.leveled(3), self.KNOWN) == "helper.py"
        assert resolve_import_target("pkg/sub/user.py", self.leveled(4), self.KNOWN) is None


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
