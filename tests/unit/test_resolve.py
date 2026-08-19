"""The resolution pass: the four tiers, their precedence, and determinism.

One fixture repository carries every tier at once, because the tiers are a
precedence rule rather than four independent classifiers: the interesting
cases are the ones where two kinds of evidence exist and the stronger has to
win. ``shadow.py`` is the case that matters most -- it imports ``helper`` and
then defines its own -- and the assertion is that the local definition wins
and the imported one does not appear on the edge at all.
"""

import pytest

from agentless_mcp.core import refs, resolve

CORE = '''\
"""Definitions the rest of the fixture resolves against."""


def helper(value):
    return value


def only_once(value):
    return value


def caller(value):
    return helper(value)
'''

USER = """\
from core import helper


def use(value):
    return helper(value)
"""

MODULE_USER = """\
import core


def use_module(value):
    return core.only_once(value)
"""

STRANGER = """\
def stray(value):
    return only_once(value)
"""

ALPHA = """\
def shared(value):
    return value
"""

BETA = """\
def shared(value):
    return value + 1
"""

GAMMA = """\
def ask(value):
    return shared(value)
"""

SHADOW = """\
from core import helper


def helper(value):
    return value * 2


def use_shadow(value):
    return helper(value)
"""

BASES = """\
class Base:
    pass


class Child(Base):
    pass
"""

DERIVED = """\
from bases import Base


class Other(Base):
    pass
"""

ISLAND = """\
def marooned(argument):
    return argument
"""

PARAM_SHADOW = """\
def shade(only_once, marker: Child = Base):
    return only_once(marker)
"""

FILES = {
    "core.py": CORE,
    "user.py": USER,
    "module_user.py": MODULE_USER,
    "stranger.py": STRANGER,
    "alpha.py": ALPHA,
    "beta.py": BETA,
    "gamma.py": GAMMA,
    "shadow.py": SHADOW,
    "bases.py": BASES,
    "derived.py": DERIVED,
    "island.py": ISLAND,
    "param_shadow.py": PARAM_SHADOW,
}

PKG_INIT = """\
def name_that_is_a_symbol(value):
    return value
"""

PKG_MOD = """\
def wrapped(value):
    return value
"""

SUBMODULE_MAIN = """\
from pkg import mod
from pkg import name_that_is_a_symbol


def use(value):
    return mod.wrapped(name_that_is_a_symbol(value))
"""

ALIASED_MAIN = """\
from pkg import mod as m


def use_alias(value):
    return m.wrapped(value)
"""

SUBMODULE_FILES = {
    "pkg/__init__.py": PKG_INIT,
    "pkg/mod.py": PKG_MOD,
    "main.py": SUBMODULE_MAIN,
    "aliased.py": ALIASED_MAIN,
}

SUBMODULE_CYCLE = {
    "pkg/__init__.py": "",
    "pkg/sub.py": "import loop\n\n\ndef in_sub():\n    return 1\n",
    "loop.py": "from pkg import sub\n\n\ndef in_loop():\n    return 2\n",
}

TWO_CYCLE = {
    "x.py": "import y\n\n\ndef in_x():\n    return 1\n",
    "y.py": "import x\n\n\ndef in_y():\n    return 2\n",
}

THREE_CYCLE = {
    "p.py": "import q\n\n\ndef in_p():\n    return 1\n",
    "q.py": "import r\n\n\ndef in_q():\n    return 2\n",
    "r.py": "import p\n\n\ndef in_r():\n    return 3\n",
}


def write(root, files):
    """Write a mapping of relative path to text under ``root``."""
    for relative, text in files.items():
        (root / relative).parent.mkdir(parents=True, exist_ok=True)
        (root / relative).write_text(text, encoding="utf-8")
    return root


def resolved(root, extractor):
    """Scan a repository and resolve every reference it holds."""
    scan = refs.scan_repo(root, extractor)
    index = refs.build_ref_index(scan)
    resolver, graph = resolve.resolve_repo(scan, index)
    return resolver, graph


@pytest.fixture
def repo(tmp_path, extractor):
    """The whole-ladder fixture, already resolved."""
    return resolved(write(tmp_path, FILES), extractor)


def edges_from(graph, source, name):
    """Every reference edge leaving ``source`` that spells ``name``."""
    return [
        edge
        for edge in graph.edges
        if edge.source.node == source
        and edge.name == name
        and edge.relation is resolve.Relation.REFERENCES
    ]


class TestTiers:
    def test_a_local_definition_resolves_same_file(self, repo):
        _, graph = repo
        edges = edges_from(graph, "py:core.py::caller", "helper")
        assert [edge.tier for edge in edges] == [resolve.Tier.SAME_FILE]
        assert edges[0].target.node == "py:core.py::helper"

    def test_a_named_import_resolves_imported(self, repo):
        _, graph = repo
        edges = edges_from(graph, "py:user.py::use", "helper")
        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]
        assert edges[0].target.node == "py:core.py::helper"

    def test_a_whole_module_import_resolves_imported(self, repo):
        _, graph = repo
        edges = edges_from(graph, "py:module_user.py::use_module", "only_once")
        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]
        assert edges[0].target.node == "py:core.py::only_once"

    def test_the_repository_s_only_definition_resolves_unique(self, repo):
        _, graph = repo
        edges = edges_from(graph, "py:stranger.py::stray", "only_once")
        assert [edge.tier for edge in edges] == [resolve.Tier.UNIQUE]

    def test_an_unimported_collision_lists_every_candidate(self, repo):
        _, graph = repo
        edges = edges_from(graph, "py:gamma.py::ask", "shared")
        assert {edge.tier for edge in edges} == {resolve.Tier.AMBIGUOUS}
        assert sorted(edge.target.node for edge in edges) == [
            "py:alpha.py::shared",
            "py:beta.py::shared",
        ]

    def test_a_local_definition_shadows_an_import_of_the_same_name(self, repo):
        _, graph = repo
        edges = edges_from(graph, "py:shadow.py::use_shadow", "helper")
        assert [edge.tier for edge in edges] == [resolve.Tier.SAME_FILE]
        assert [edge.target.node for edge in edges] == ["py:shadow.py::helper"]

    def test_the_tier_labels_are_the_ones_rendered(self):
        assert resolve.Tier.SAME_FILE.label == "same-file"
        assert resolve.Tier.IMPORTED.label == "resolved-via-import"
        assert resolve.Tier.UNIQUE.label == "unique"
        assert resolve.Tier.AMBIGUOUS.label == "name-only-ambiguous"

    def test_a_name_nothing_defines_resolves_to_nothing(self, repo):
        resolver, _ = repo
        assert resolver.resolve("no_such_name_anywhere", "core.py") is None


class TestLocalBindings:
    def test_a_parameter_named_after_a_unique_symbol_produces_no_edge(self, repo):
        _, graph = repo
        assert edges_from(graph, "py:param_shadow.py::shade", "only_once") == []

    def test_the_same_name_not_locally_bound_still_resolves(self, repo):
        _, graph = repo
        edges = edges_from(graph, "py:stranger.py::stray", "only_once")
        assert [edge.tier for edge in edges] == [resolve.Tier.UNIQUE]
        assert edges[0].target.node == "py:core.py::only_once"

    def test_annotation_and_default_names_on_a_parameter_still_resolve(self, repo):
        _, graph = repo
        annotation = edges_from(graph, "py:param_shadow.py::shade", "Child")
        default = edges_from(graph, "py:param_shadow.py::shade", "Base")
        assert [edge.target.node for edge in annotation] == ["py:bases.py::Child"]
        assert [edge.target.node for edge in default] == ["py:bases.py::Base"]


class TestRelations:
    def test_a_base_class_in_the_same_file_is_an_inherits_edge(self, repo):
        _, graph = repo
        inherits = [
            edge
            for edge in graph.edges
            if edge.relation is resolve.Relation.INHERITS
            and edge.source.node == "py:bases.py::Child"
        ]
        assert [(edge.target.node, edge.tier) for edge in inherits] == [
            ("py:bases.py::Base", resolve.Tier.SAME_FILE)
        ]

    def test_an_imported_base_class_is_an_inherits_edge(self, repo):
        _, graph = repo
        inherits = [
            edge
            for edge in graph.edges
            if edge.relation is resolve.Relation.INHERITS
            and edge.source.node == "py:derived.py::Other"
        ]
        assert [(edge.target.node, edge.tier) for edge in inherits] == [
            ("py:bases.py::Base", resolve.Tier.IMPORTED)
        ]

    def test_imports_are_file_level_edges(self, repo):
        _, graph = repo
        pairs = {(edge.source.node, edge.target.node) for edge in graph.import_edges()}
        assert ("user.py", "core.py") in pairs
        assert ("derived.py", "bases.py") in pairs

    def test_a_module_level_reference_is_attributed_to_its_file(self, repo):
        _, graph = repo
        edges = edges_from(graph, "user.py", "helper")
        assert edges
        assert all(not edge.source.is_symbol for edge in edges)

    @pytest.mark.parametrize("text", ["Generic[T]", " enum.Enum ", "Base"])
    def test_base_expressions_reduce_to_a_lookup_name(self, text):
        assert resolve.base_name(text) in {"Generic", "Enum", "Base"}

    def test_a_keyword_in_a_base_list_is_not_a_base(self):
        assert resolve.base_name("metaclass=ABCMeta") == ""


class TestSubmoduleImports:
    """``from pkg import mod`` binds ``mod`` as a module when ``pkg.mod`` is one.

    The submodule file, not the package's ``__init__``, is what the name
    reaches -- so the import edge, the scope binding and the reference tier
    all have to land on ``pkg/mod.py``. A name the dotted probe cannot match
    to a file stays what it always was: a symbol imported from the module.
    """

    @pytest.fixture
    def submodule_repo(self, tmp_path, extractor):
        return resolved(write(tmp_path, SUBMODULE_FILES), extractor)

    def test_a_from_import_of_a_submodule_is_an_import_edge(self, submodule_repo):
        _, graph = submodule_repo
        pairs = {(edge.source.node, edge.target.node) for edge in graph.import_edges()}
        assert ("main.py", "pkg/mod.py") in pairs

    def test_the_submodule_name_binds_to_the_submodule_file(self, submodule_repo):
        resolver, _ = submodule_repo
        scope = resolver.scopes["main.py"]
        assert scope.named["mod"] == frozenset({"pkg/mod.py"})
        assert "pkg/mod.py" in scope.modules

    def test_a_reference_through_the_submodule_resolves_imported(self, submodule_repo):
        _, graph = submodule_repo
        edges = edges_from(graph, "py:main.py::use", "wrapped")
        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]
        assert edges[0].target.node == "py:pkg/mod.py::wrapped"

    def test_a_from_import_of_a_symbol_still_binds_the_symbol(self, submodule_repo):
        resolver, graph = submodule_repo
        scope = resolver.scopes["main.py"]
        assert scope.named["name_that_is_a_symbol"] == frozenset({"pkg/__init__.py"})
        edges = edges_from(graph, "py:main.py::use", "name_that_is_a_symbol")
        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]
        assert edges[0].target.node == "py:pkg/__init__.py::name_that_is_a_symbol"

    def test_an_aliased_submodule_import_keeps_the_edge(self, submodule_repo):
        resolver, graph = submodule_repo
        pairs = {(edge.source.node, edge.target.node) for edge in graph.import_edges()}
        assert ("aliased.py", "pkg/mod.py") in pairs
        assert resolver.scopes["aliased.py"].named["mod"] == frozenset({"pkg/mod.py"})
        edges = edges_from(graph, "py:aliased.py::use_alias", "wrapped")
        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]

    def test_a_cycle_through_a_submodule_import_is_found(self, tmp_path, extractor):
        _, graph = resolved(write(tmp_path, SUBMODULE_CYCLE), extractor)
        cycles = resolve.import_cycles(graph)
        assert [cycle.files for cycle in cycles] == [("loop.py", "pkg/sub.py")]


class TestDeterminism:
    def test_two_full_resolves_of_one_tree_are_identical(self, tmp_path, extractor):
        root = write(tmp_path, FILES)
        first = resolved(root, extractor)[1]
        second = resolved(root, extractor)[1]
        assert [edge.sort_key for edge in first.edges] == [edge.sort_key for edge in second.edges]
        assert repr(first.edges) == repr(second.edges)

    def test_identical_edges_are_reported_once(self, tmp_path, extractor):
        repeated = "def a():\n    return 1\n\n\ndef b():\n    return a() + a() + a()\n"
        root = write(tmp_path, {"repeat.py": repeated})
        _, graph = resolved(root, extractor)
        assert len(edges_from(graph, "py:repeat.py::b", "a")) == 1


class TestPaths:
    def test_a_connected_pair_reports_its_hops(self, repo):
        _, graph = repo
        found = resolve.shortest_path(graph, "py:user.py::use", "py:core.py::helper")
        assert found.found
        assert [hop.arrival.node for hop in found.hops] == ["py:core.py::helper"]
        assert found.hops[0].forward

    def test_an_unconnected_pair_is_an_answer_not_an_error(self, repo):
        _, graph = repo
        found = resolve.shortest_path(graph, "py:island.py::marooned", "py:core.py::helper")
        assert not found.found
        assert not found.exhausted
        assert found.hops == ()

    def test_ambiguous_edges_are_excluded_by_default(self, repo):
        _, graph = repo
        excluded = resolve.shortest_path(graph, "py:gamma.py::ask", "py:alpha.py::shared")
        included = resolve.shortest_path(
            graph, "py:gamma.py::ask", "py:alpha.py::shared", include_ambiguous=True
        )
        assert not excluded.found
        assert included.found
        assert [hop.arrival.node for hop in included.hops] == ["py:alpha.py::shared"]

    def test_a_path_is_walked_in_both_directions(self, repo):
        _, graph = repo
        found = resolve.shortest_path(graph, "py:core.py::helper", "py:user.py::use")
        assert found.found
        assert not found.hops[-1].forward

    def test_a_node_reaches_itself_in_no_hops(self, repo):
        _, graph = repo
        found = resolve.shortest_path(graph, "py:core.py::helper", "py:core.py::helper")
        assert found.found
        assert found.hops == ()

    def test_the_search_bound_is_reported_rather_than_answered_as_no_path(self, repo):
        _, graph = repo
        reachable = resolve.shortest_path(graph, "py:user.py::use", "py:core.py::caller")
        assert reachable.found
        found = resolve.shortest_path(graph, "py:user.py::use", "py:core.py::caller", max_visited=1)
        assert not found.found
        assert found.exhausted


class TestCycles:
    def test_a_repository_without_cycles_reports_none(self, repo):
        _, graph = repo
        assert resolve.import_cycles(graph) == ()

    def test_a_two_file_cycle_is_found(self, tmp_path, extractor):
        _, graph = resolved(write(tmp_path, TWO_CYCLE), extractor)
        cycles = resolve.import_cycles(graph)
        assert [cycle.files for cycle in cycles] == [("x.py", "y.py")]
        assert cycles[0].chain == "x.py -> y.py -> x.py"

    def test_a_three_file_cycle_is_found_as_a_real_walk(self, tmp_path, extractor):
        _, graph = resolved(write(tmp_path, THREE_CYCLE), extractor)
        cycles = resolve.import_cycles(graph)
        assert [cycle.files for cycle in cycles] == [("p.py", "q.py", "r.py")]
        assert cycles[0].chain == "p.py -> q.py -> r.py -> p.py"

    def test_two_cycles_are_reported_shortest_first(self, tmp_path, extractor):
        _, graph = resolved(write(tmp_path, {**TWO_CYCLE, **THREE_CYCLE}), extractor)
        cycles = resolve.import_cycles(graph)
        assert [len(cycle.files) for cycle in cycles] == [2, 3]

    def test_the_cycle_order_does_not_depend_on_the_run(self, tmp_path, extractor):
        root = write(tmp_path, {**TWO_CYCLE, **THREE_CYCLE})
        first = resolve.import_cycles(resolved(root, extractor)[1])
        second = resolve.import_cycles(resolved(root, extractor)[1])
        assert first == second
