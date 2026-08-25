"""The reference graph and the personalized PageRank over it."""

import math
from contextlib import contextmanager
from dataclasses import replace
from types import MappingProxyType

import pytest

from agentless_mcp.core import graph as graph_module
from agentless_mcp.core.graph import (
    AMBIGUOUS_MATCH_MULTIPLIER,
    DEFAULT_MAX_ITERATIONS,
    IMPORT_EDGE_WEIGHT,
    NOISE_NAME_MULTIPLIER,
    RELATION_WEIGHTS,
    UNIQUE_MATCH_MULTIPLIER,
    Convergence,
    PathIndex,
    Reached,
    RefGraph,
    build_graph,
    flood,
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


# A `src/` layout, where a package is imported by a name that matches no path
# from the repository root and only the tail search can answer it.
SRC_LAYOUT = {
    "src/store/__init__.py": "from store.ledger import post\n",
    "src/store/ledger.py": "def post(entry):\n    return entry\n",
    "src/store/audit/__init__.py": "",
    "src/store/audit/trail.py": (
        "from store import ledger\nfrom store.audit import trail\n\n\n"
        "def keep(entry):\n    return ledger.post(entry)\n"
    ),
    "app.py": "from store.audit import trail\n\n\ndef run(entry):\n    return trail.keep(entry)\n",
}


@contextmanager
def scanned_path_set():
    """Run ``build_graph`` against a plain path set instead of the tail index.

    `resolve_import_target` documents two argument shapes that must answer the
    same question, and this is how the suite reaches the one `build_graph` does
    not choose. Restored on the way out, so nothing leaks into another test.
    """
    original = graph_module.PathIndex

    class _PlainSet:
        @classmethod
        def build(cls, paths):
            return frozenset(paths)

    graph_module.PathIndex = _PlainSet
    try:
        yield
    finally:
        graph_module.PathIndex = original


def js_import(module):
    """A JavaScript-style relative import statement of ``module``."""
    return ImportStatement(
        module=module,
        names=(),
        is_relative=True,
        relative_level=0,
        line_number=1,
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

        A two-node line will not do it any more. Backflow makes ``a.py`` and
        ``b.py`` a mutual pair, and a mutual pair splits evenly whatever the
        damping, so the fixture would pin nothing. A fork does: ``a.py``
        references ``b.py`` and ``c.py``, each of which reaches only back to
        ``a.py``, so by symmetry ``b.py`` and ``c.py`` are level and ``a.py``
        settles at ``((1 - damping) / 3 + damping) / (1 + damping)`` -- which
        is 0.486486 at 0.85 and moves for any other damping.
        """
        graph = RefGraph(
            nodes=("a.py", "b.py", "c.py"),
            edges=line(("a.py", "b.py", 1.0), ("a.py", "c.py", 1.0)),
        )
        rank = personalized_pagerank(graph).rank

        assert math.isclose(rank["a.py"], 0.4864865, abs_tol=1e-6)
        assert math.isclose(rank["b.py"], 0.2567568, abs_tol=1e-6)
        assert math.isclose(rank["c.py"], 0.2567568, abs_tol=1e-6)


class TestTheWalkStepsBothWays:
    """Backflow, and the one kind of file it must not reach."""

    def test_a_referrer_of_the_seed_outranks_a_file_unrelated_to_it(self):
        """The defect this exists for: a caller scored as low as a stranger.

        ``caller.py`` references the seed and ``island.py`` has no edge at
        all. A forward-only walk scores both of them at exactly zero, because
        rank flows only toward a definer and neither file is one -- so the
        ranking past the seed was decided by files the seed *imports*, and
        the one file that uses it could not be told from an unconnected one.
        """
        graph = RefGraph(
            nodes=("seed.py", "caller.py", "hub.py", "island.py"),
            edges=line(("seed.py", "hub.py", 1.0), ("caller.py", "seed.py", 1.0)),
        )
        rank = personalized_pagerank(graph, {"seed.py": 1.0}).rank

        assert rank_order(rank)[0] == "seed.py"
        assert rank["caller.py"] > 0.0
        assert math.isclose(rank["island.py"], 0.0, abs_tol=1e-9)

    def test_a_pure_source_does_not_gain_from_the_files_it_references(self):
        """A test suite references many files; that never makes it the answer."""
        edges = line(
            ("tests/test_all.py", "alpha.py", 1.0),
            ("tests/test_all.py", "bravo.py", 1.0),
            ("tests/test_all.py", "chose.py", 1.0),
        )
        nodes = ("alpha.py", "bravo.py", "chose.py", "tests/test_all.py")
        graph = RefGraph(nodes=nodes, edges=edges)

        unguarded = personalized_pagerank(graph).rank
        guarded = personalized_pagerank(graph, pure_sources={"tests/test_all.py"}).rank

        assert rank_order(unguarded)[0] == "tests/test_all.py"
        assert rank_order(guarded)[0] != "tests/test_all.py"
        assert guarded["tests/test_all.py"] < unguarded["tests/test_all.py"]

    def test_the_graph_itself_keeps_its_direction(self):
        """Only the walk reads the graph both ways; the edge map does not."""
        graph = RefGraph(nodes=("a.py", "b.py"), edges=line(("a.py", "b.py", 1.0)))

        assert graph.adjacency()["b.py"] == ()
        assert graph.reverse_adjacency()["a.py"] == ()


class TestSeedsLeadTheOrder:
    """A resolved seed is the file the caller named, so it opens the answer."""

    def test_a_seed_leads_even_when_the_walk_ranks_another_file_higher(self):
        rank = {"hub.py": 0.6, "seed.py": 0.3, "leaf.py": 0.1}

        assert rank_order(rank, {"seed.py"}) == ["seed.py", "hub.py", "leaf.py"]

    def test_seeds_keep_the_walk_order_among_themselves(self):
        rank = {"hub.py": 0.6, "low.py": 0.1, "high.py": 0.3}

        assert rank_order(rank, {"low.py", "high.py"}) == ["high.py", "low.py", "hub.py"]

    def test_no_seeds_is_the_plain_ranking(self):
        rank = {"a.py": 0.2, "b.py": 0.5}

        assert rank_order(rank) == rank_order(rank, frozenset()) == ["b.py", "a.py"]


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

        ranking = personalized_pagerank(graph, damping=0.99, limits=Convergence(max_iterations=500))

        assert ranking.converged
        assert ranking.iterations == 252


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
        """``build_graph`` hands the index over; the whole edge map must not move.

        The index replaced a scan that allocated a ``PurePosixPath`` per known
        path per imported name, and the claim behind that replacement is that
        the edge set is unchanged -- not that it is similar, and not that one
        edge of it survived. Asserting one edge above a weight floor would pass
        with every other edge dropped, with every weight changed, and with
        extra edges added, so the whole mapping is compared.

        Both weights and endpoints: a tail that resolves to a different file
        moves an edge, and a name counted twice moves only a number.
        """
        root = write(tmp_path, PACKAGE | SRC_LAYOUT)
        scan = scan_repo(root, extractor)
        index = build_ref_index(scan)

        through_index = build_graph(scan, index)
        with scanned_path_set():
            through_scan = build_graph(scan, index)

        assert dict(through_index.edges) == dict(through_scan.edges)
        assert through_index.nodes == through_scan.nodes
        # The comparison is only worth what the fixture exercises, so the
        # fixture has to produce edges at all.
        assert through_index.edges


class TestPackageEntryPoints:
    """A package is imported by its directory's name, not by its file's.

    `from agentless_mcp.application import X` names
    `src/agentless_mcp/application/__init__.py`. The direct-candidate loop
    builds that path from the repository root, which is one directory short in
    a `src/` layout, and the tail search compared the module against the file's
    own stem -- `.../application/__init__` -- which the module string never
    ends with. So every package-level import in a `src/` layout resolved to
    nothing: 115 statements on this repository, each an edge missing from the
    import graph and each able to hide a cycle routed through an `__init__.py`.
    """

    PACKAGED = (
        "src/store/__init__.py",
        "src/store/ledger.py",
        "src/store/audit/__init__.py",
    )

    def package_import(self, module):
        return ImportStatement(
            module=module,
            names=("post",),
            is_relative=False,
            relative_level=0,
            line_number=1,
        )

    def test_a_single_segment_package_still_names_no_tail(self):
        """The one package shape the tail rule cannot answer, stated as a limit.

        A module string with no separator has no tail that can land on a path
        boundary, so `import store` cannot be told from a `store` anywhere in
        the tree -- see :func:`_module_tail`. Six statements on this repository
        are in this position, all of them `import agentless_mcp`. Widening the
        rule to reach them would let `refs` claim `src/mycore/refs.py`, which
        is the guess this resolver refuses to make.
        """
        assert resolve_import_target("app.py", self.package_import("store"), self.PACKAGED) is None

    def test_a_nested_package_import_resolves_to_its_entry_point(self):
        assert (
            resolve_import_target("app.py", self.package_import("store.audit"), self.PACKAGED)
            == "src/store/audit/__init__.py"
        )

    def test_a_module_still_beats_the_package_that_holds_it(self):
        # `store.ledger` names a module, and the entry point of the package it
        # sits in must not answer for it.
        assert (
            resolve_import_target("app.py", self.package_import("store.ledger"), self.PACKAGED)
            == "src/store/ledger.py"
        )

    def test_the_table_answers_a_package_the_same_way_the_scan_does(self):
        index = PathIndex.build(self.PACKAGED)
        for module in ("store", "store.audit", "store.ledger", "store.missing"):
            probe = self.package_import(module)
            assert resolve_import_target("app.py", probe, index) == resolve_import_target(
                "app.py", probe, list(self.PACKAGED)
            ), module

    def test_an_ecmascript_directory_index_answers_for_its_directory(self):
        # The same rule reaches `index.ts`, which is where the entry-point
        # stems come from: they are derived from `_MODULE_SUFFIXES`.
        known = ("packages/ui/src/index.ts", "packages/ui/src/button.ts")
        statement = ImportStatement(
            module="ui/src",
            names=(),
            is_relative=False,
            relative_level=0,
            line_number=1,
        )

        assert resolve_import_target("app.ts", statement, known) == "packages/ui/src/index.ts"

    def test_a_package_import_becomes_a_graph_edge(self, tmp_path, extractor):
        graph = graph_of(write(tmp_path, SRC_LAYOUT), extractor)

        assert ("app.py", "src/store/audit/__init__.py") in graph.edges


class TestImportResolution:
    def test_a_relative_javascript_import_resolves_to_a_sibling(self):
        statement = ImportStatement(
            module="./pricing",
            names=(),
            is_relative=True,
            relative_level=0,
            line_number=1,
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


# A reference graph with one cycle and two test files, hand-built because the
# flood is about shape rather than about parsing. Edges run referrer to
# definer, so every test file is a pure source: `src/a.py` reaches nothing
# back, which is the whole reason the backward walk exists.
FLOODED = RefGraph(
    nodes=(
        "lonely.py",
        "src/a.py",
        "src/b.py",
        "src/c.py",
        "tests/test_a.py",
        "tests/test_b.py",
    ),
    edges=line(
        ("tests/test_a.py", "src/a.py", 3.0),
        ("tests/test_b.py", "src/b.py", 3.0),
        ("src/b.py", "src/a.py", 1.0),
        ("src/c.py", "src/b.py", 1.0),
        ("src/a.py", "src/c.py", 1.0),
    ),
)


class TestReverseAdjacency:
    def test_it_lists_the_files_that_mention_each_file(self):
        """The forward index cannot answer this, and the flood is built on it.

        `adjacency` groups an edge under its source. Grouping the same edge
        map under its target is the only new data the backward walk needs, and
        getting the direction wrong would make a backward flood a forward one
        that still returns rows.
        """
        incoming = FLOODED.reverse_adjacency()

        assert incoming["src/a.py"] == (("src/b.py", 1.0), ("tests/test_a.py", 3.0))

    def test_a_file_nothing_mentions_is_present_and_empty(self):
        # Absent and empty are different answers, and a caller indexing the
        # table by node must not meet a KeyError for a file with no fan-in.
        assert FLOODED.reverse_adjacency()["lonely.py"] == ()

    def test_every_node_appears_exactly_once_in_each_direction(self):
        assert set(FLOODED.reverse_adjacency()) == set(FLOODED.adjacency()) == set(FLOODED.nodes)


class TestFlood:
    """Depth-bounded reachability over the reference graph, either direction."""

    def test_backward_finds_the_tests_that_reach_a_file(self):
        """The feature this exists for: production code has no outbound edge
        to the tests that exercise it, so only the backward walk finds them.
        """
        walked = flood(FLOODED, ("src/a.py",), backward=True)

        assert walked.reached == (
            Reached(path="src/b.py", depth=1),
            Reached(path="tests/test_a.py", depth=1),
            Reached(path="src/c.py", depth=2),
            Reached(path="tests/test_b.py", depth=2),
        )
        assert walked.exhausted is False

    def test_forward_walks_the_other_way(self):
        # Same graph, same seed, opposite direction: a result that did not
        # move would mean `backward` is being ignored.
        walked = flood(FLOODED, ("src/a.py",))

        assert walked.reached == (
            Reached(path="src/c.py", depth=1),
            Reached(path="src/b.py", depth=2),
        )

    def test_a_seed_is_never_its_own_answer(self):
        """A seed is the question. `src/a.py` is reachable from itself around
        the cycle, and reporting it would put the question in the answer.
        """
        walked = flood(FLOODED, ("src/a.py", "src/b.py"), backward=True)

        assert {row.path for row in walked.reached}.isdisjoint({"src/a.py", "src/b.py"})

    def test_a_cycle_terminates_and_keeps_the_shortest_arrival(self):
        """`c` is one hop from `a` directly and two hops around the cycle.

        A walk that re-expanded an already-seen node would not return at all
        here, and one that overwrote the depth would report `c` at 2.
        """
        cyclic = RefGraph(
            nodes=("a.py", "b.py", "c.py"),
            edges=line(
                ("a.py", "b.py", 1.0),
                ("b.py", "c.py", 1.0),
                ("c.py", "a.py", 1.0),
                ("a.py", "c.py", 1.0),
            ),
        )

        walked = flood(cyclic, ("a.py",))

        assert walked.reached == (
            Reached(path="b.py", depth=1),
            Reached(path="c.py", depth=1),
        )

    def test_rows_are_ordered_by_depth_then_path(self):
        """The port orders by `(depth, name)`, which is not a total order.

        This fixture discovers `z_far.py` before `a_far.py`, because the
        frontier node that reaches `z_far.py` sorts first. Insertion order and
        path order therefore disagree at depth 2, and only a sort on
        `(depth, path)` makes two runs of an unchanged graph agree.
        """
        forked = RefGraph(
            nodes=("a_far.py", "b_mid.py", "c_mid.py", "seed.py", "z_far.py"),
            edges=line(
                ("seed.py", "b_mid.py", 1.0),
                ("seed.py", "c_mid.py", 1.0),
                ("b_mid.py", "z_far.py", 1.0),
                ("c_mid.py", "a_far.py", 1.0),
            ),
        )

        walked = flood(forked, ("seed.py",))

        assert [(row.depth, row.path) for row in walked.reached] == [
            (1, "b_mid.py"),
            (1, "c_mid.py"),
            (2, "a_far.py"),
            (2, "z_far.py"),
        ]

    def test_the_depth_bound_stops_the_walk_without_calling_it_exhausted(self):
        # A depth bound is the caller's question, not a failure to answer it.
        walked = flood(FLOODED, ("src/a.py",), backward=True, max_depth=1)

        assert [row.path for row in walked.reached] == ["src/b.py", "tests/test_a.py"]
        assert walked.exhausted is False

    def test_a_walk_that_hits_the_visit_bound_says_so(self):
        """Nothing further reaches this, and the walk stopped looking, are
        different answers. A truncated reach set read as a complete one says a
        file is unrelated when nobody checked.
        """
        walked = flood(FLOODED, ("src/a.py",), backward=True, max_visited=1)

        assert walked.exhausted is True
        assert "tests/test_b.py" not in {row.path for row in walked.reached}

    def test_a_seed_naming_no_node_reaches_nothing(self):
        walked = flood(FLOODED, ("gone.py",), backward=True)

        assert walked.reached == ()
        assert walked.visited == 0
        assert walked.exhausted is False

    def test_an_unknown_seed_beside_a_known_one_changes_nothing(self):
        # The unknown name must not shift the answer the known seed gives, and
        # must not raise from the adjacency lookup either.
        assert flood(FLOODED, ("src/a.py", "gone.py"), backward=True) == flood(
            FLOODED, ("src/a.py",), backward=True
        )

    def test_a_repeated_seed_is_walked_once(self):
        assert (
            flood(FLOODED, ("src/a.py", "src/a.py"), backward=True).visited
            == flood(FLOODED, ("src/a.py",), backward=True).visited
        )


# A base class in one file and its subclass in another, which is the only
# shape the inheritance weight can act on: `ASTSymbol.bases` is populated by
# the Python class handler and by nothing else.
INHERITED = {
    "base.py": "class Ledger:\n    def post(self):\n        return 1\n",
    "derived.py": "class Journal(Ledger):\n    pass\n",
}


class TestRelationWeights:
    """The ported relation-typed weight table, and the flag that gates it.

    The table is dark on purpose. `ASTSymbol.bases` is filled in by exactly one
    extractor handler -- the Python class handler -- so the inheritance weight
    fires for Python and silently never fires for any other language.
    """

    def built(self, tmp_path, extractor, files, *, relation_weights):
        scan = scan_repo(write(tmp_path, files), extractor)
        return build_graph(scan, build_ref_index(scan), relation_weights=relation_weights)

    def test_the_shipped_default_is_the_shipped_weighting(self, tmp_path, extractor):
        """The flag must be inert until a caller asks for it.

        The characterization goldens pin the map byte for byte, so a default
        that quietly re-weighted anything would move output nobody asked to
        move.
        """
        files = INHERITED | {"user.py": "import base\n"}
        scan = scan_repo(write(tmp_path, files), extractor)
        index = build_ref_index(scan)

        assert dict(build_graph(scan, index).edges) == dict(
            build_graph(scan, index, relation_weights=False).edges
        )
        assert build_graph(scan, index).edges[("user.py", "base.py")] == IMPORT_EDGE_WEIGHT

    def test_an_import_falls_to_the_tables_import_weight(self, tmp_path, extractor):
        # An import statement mints no reference edge, so this edge is the
        # import weight alone and the swap is visible on its own.
        graph = self.built(
            tmp_path,
            extractor,
            {"base.py": "VALUE = 1\n", "user.py": "import base\n"},
            relation_weights=True,
        )

        assert graph.edges[("user.py", "base.py")] == RELATION_WEIGHTS["imports"]

    def test_a_declared_base_class_adds_the_inheritance_weight(self, tmp_path, extractor):
        """`class Journal(Ledger)` is a stronger claim than mentioning the name.

        Measured as a delta against the same graph unweighted, because the
        base name is also an ordinary reference and that contribution is
        present either way.
        """
        without = self.built(tmp_path / "off", extractor, INHERITED, relation_weights=False)
        with_bases = self.built(tmp_path / "on", extractor, INHERITED, relation_weights=True)

        edge = ("derived.py", "base.py")
        assert with_bases.edges[edge] - without.edges[edge] == RELATION_WEIGHTS["inheritance"]

    def test_a_qualified_or_subscripted_base_still_resolves(self, tmp_path, extractor):
        """Bases arrive as source text: `pkg.Ledger` and `Ledger[int]`.

        Handed to the name index unnormalized, neither resolves, and the
        weighting would fire only for the plainest spelling of the one
        language it works for at all.
        """
        files = {
            "base.py": "class Ledger:\n    def post(self):\n        return 1\n",
            "derived.py": "class One(base.Ledger):\n    pass\n\n\nclass Two(Ledger[int]):\n"
            "    pass\n",
        }
        without = self.built(tmp_path / "off", extractor, files, relation_weights=False)
        with_bases = self.built(tmp_path / "on", extractor, files, relation_weights=True)

        edge = ("derived.py", "base.py")
        assert with_bases.edges[edge] - without.edges[edge] == 2 * RELATION_WEIGHTS["inheritance"]

    def test_a_keyword_argument_in_a_base_list_is_not_a_base(self, tmp_path, extractor):
        """`metaclass=Meta` is recorded among the bases and is not one.

        Counting it would weight a metaclass as a superclass, and the same
        mistake would weight any other keyword argument a base list carries.
        """
        files = {
            "meta.py": "class Meta(type):\n    pass\n",
            "derived.py": "class Odd(metaclass=Meta):\n    pass\n",
        }
        without = self.built(tmp_path / "off", extractor, files, relation_weights=False)
        with_bases = self.built(tmp_path / "on", extractor, files, relation_weights=True)

        edge = ("derived.py", "meta.py")
        assert with_bases.edges[edge] == without.edges[edge]

    def test_a_base_defined_in_the_same_file_points_nowhere(self, tmp_path, extractor):
        # The shadowing rule the reference pass applies to every name: a local
        # definition wins, so no cross-file inheritance edge is minted.
        files = {
            "solo.py": "class Ledger:\n    pass\n\n\nclass Journal(Ledger):\n    pass\n",
            "other.py": "class Ledger:\n    pass\n",
        }
        graph = self.built(tmp_path, extractor, files, relation_weights=True)

        assert ("solo.py", "other.py") not in graph.edges
