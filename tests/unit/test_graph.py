"""The reference graph and the personalized PageRank over it."""

import math

from agentless_mcp.core.graph import (
    DEFAULT_DAMPING,
    IMPORT_EDGE_WEIGHT,
    NOISE_NAME_MULTIPLIER,
    RefGraph,
    build_graph,
    name_multiplier,
    personalized_pagerank,
    rank_order,
    resolve_import_target,
)
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.refs import build_ref_index, scan_repo


def line(*paths_and_weights):
    """Build an edge map from ``(source, target, weight)`` triples."""
    return {(source, target): weight for source, target, weight in paths_and_weights}


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

    def test_two_runs_produce_identical_numbers(self):
        graph = RefGraph(
            nodes=("a.py", "b.py", "c.py"),
            edges=line(("a.py", "b.py", 2.0), ("b.py", "c.py", 1.0), ("c.py", "a.py", 0.5)),
        )
        assert personalized_pagerank(graph) == personalized_pagerank(graph)

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

    def test_the_default_damping_is_the_documented_one(self):
        assert DEFAULT_DAMPING == 0.85


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
