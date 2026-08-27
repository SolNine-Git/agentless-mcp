"""The resolution pass: the four tiers, their precedence, and determinism.

One fixture repository carries every tier at once, because the tiers are a
precedence rule rather than four independent classifiers: the interesting
cases are the ones where two kinds of evidence exist and the stronger has to
win. ``shadow.py`` is the case that matters most -- it imports ``helper`` and
then defines its own -- and the assertion is that the local definition wins
and the imported one does not appear on the edge at all.
"""

import pytest

from agentless_mcp.core import grammars, refs, resolve, symbols

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

ALIASED_MODULE = """\
import core as c


def use_alias(value):
    return c.only_once(value)
"""

SUBMODULE_FILES = {
    "pkg/__init__.py": PKG_INIT,
    "pkg/mod.py": PKG_MOD,
    "main.py": SUBMODULE_MAIN,
    "aliased.py": ALIASED_MAIN,
}

# A class member spells a name the module scope does not bind. Python's own
# scoping never lets `helper(value)` find `Cache.helper`, so neither may the
# resolver -- and the declared import is the answer it must not discard.
METHOD_SHADOW = {
    "core.py": "def helper(value):\n    return value\n",
    "user.py": (
        "from core import helper\n"
        "\n"
        "\n"
        "class Cache:\n"
        "    def helper(self):\n"
        "        return 0\n"
        "\n"
        "\n"
        "def use(value):\n"
        "    return helper(value)\n"
    ),
}

# The two Python relative-import forms, in a package laid out the usual way.
RELATIVE_PACKAGE = {
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

    def test_an_aliased_whole_module_import_resolves_imported(self, tmp_path, extractor):
        _, graph = resolved(
            write(tmp_path, {"core.py": CORE, "aliased_module.py": ALIASED_MODULE}),
            extractor,
        )

        edges = edges_from(graph, "py:aliased_module.py::use_alias", "only_once")
        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]

    def test_nested_attribute_name_does_not_resolve_as_a_bare_symbol(self, tmp_path, extractor):
        _, graph = resolved(
            write(
                tmp_path,
                {
                    "runtime.py": 'import sys\n\n\ndef emit():\n    sys.stderr.write("x")\n',
                    "unrelated.py": "def write(value):\n    return value\n",
                },
            ),
            extractor,
        )

        assert edges_from(graph, "py:runtime.py::emit", "write") == []

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

    def test_an_import_declaration_is_not_a_symbol_reference(self, repo):
        _, graph = repo
        edges = edges_from(graph, "user.py", "helper")
        assert edges == []

    @pytest.mark.parametrize("text", ["Generic[T]", " enum.Enum ", "Base"])
    def test_base_expressions_reduce_to_a_lookup_name(self, text):
        assert symbols.base_name(text) in {"Generic", "Enum", "Base"}

    def test_a_keyword_in_a_base_list_is_not_a_base(self):
        assert symbols.base_name("metaclass=ABCMeta") == ""


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
        assert scope.module_bindings["mod"] == frozenset({"pkg/mod.py"})

    def test_the_submodule_is_not_wholesale_evidence(self, submodule_repo):
        """`from pkg import mod` binds `mod`, not everything `mod` defines.

        The submodule used to be added to the whole-module set as well, which
        made every name `pkg/mod.py` defines resolve at `resolved-via-import`
        in a file that imported only the module object. A bare reference to
        one of them is a NameError in Python.
        """
        resolver, _ = submodule_repo
        assert "pkg/mod.py" not in resolver.scopes["main.py"].wholesale

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
        # `m`, not `mod`: `from pkg import mod as m` binds one name in this
        # file and it is the alias. Keyed on `mod` before stage 6c, which is a
        # name aliased.py never binds.
        assert resolver.scopes["aliased.py"].named["m"] == frozenset({"pkg/mod.py"})
        edges = edges_from(graph, "py:aliased.py::use_alias", "wrapped")
        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]

    def test_an_unaliased_dotted_import_binds_the_package_not_the_submodule(
        self, tmp_path, extractor
    ):
        """`import pk.deep` binds `pk`, so `pk.shared()` is a call into the package.

        Reported as `pk/deep.py::shared` at `resolved-via-import` -- one
        confidently wrong answer at the tier a caller is told to read as a
        caller, with the file Python would actually reach never listed.
        """
        root = write(
            tmp_path,
            {
                "pk/__init__.py": "def shared():\n    return 1\n",
                "pk/deep.py": "def shared():\n    return 2\n",
                "app.py": "import pk.deep\n\n\ndef go():\n    return pk.shared()\n",
            },
        )
        resolver, graph = resolved(root, extractor)
        assert resolver.scopes["app.py"].module_bindings["pk"] == frozenset({"pk/__init__.py"})

        edges = edges_from(graph, "py:app.py::go", "shared")
        assert [edge.target.node for edge in edges] == ["py:pk/__init__.py::shared"]
        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]

    def test_an_aliased_dotted_import_binds_the_submodule(self, tmp_path, extractor):
        """`import pk.deep as d` binds `d` to the submodule, and `pk` to nothing."""
        root = write(
            tmp_path,
            {
                "pk/__init__.py": "def shared():\n    return 1\n",
                "pk/deep.py": "def shared():\n    return 2\n",
                "app.py": "import pk.deep as d\n\n\ndef go():\n    return d.shared()\n",
            },
        )
        resolver, graph = resolved(root, extractor)
        bindings = resolver.scopes["app.py"].module_bindings
        assert bindings["d"] == frozenset({"pk/deep.py"})
        assert "pk" not in bindings

        edges = edges_from(graph, "py:app.py::go", "shared")
        assert [edge.target.node for edge in edges] == ["py:pk/deep.py::shared"]

    def test_a_cycle_through_a_submodule_import_is_found(self, tmp_path, extractor):
        _, graph = resolved(write(tmp_path, SUBMODULE_CYCLE), extractor)
        cycles = resolve.import_cycles(graph)
        assert [cycle.files for cycle in cycles] == [("loop.py", "pkg/sub.py")]


class TestModuleScope:
    def test_a_method_does_not_shadow_a_declared_import(self, tmp_path, extractor):
        """``same_file`` means "this file's module scope defines it", not "this file does".

        A bare call cannot reach a class member in any language this package
        parses, so a method must not outrank -- let alone suppress -- the
        candidate the file's own import declares.
        """
        _, graph = resolved(write(tmp_path, METHOD_SHADOW), extractor)
        edges = edges_from(graph, "py:user.py::use", "helper")

        assert [edge.target.node for edge in edges] == ["py:core.py::helper"]
        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]


class TestRelativeImports:
    def test_a_bare_relative_import_binds_the_sibling_submodule(self, tmp_path, extractor):
        resolver, _ = resolved(write(tmp_path, RELATIVE_PACKAGE), extractor)
        scope = resolver.scopes["pkg/sub/user.py"]

        assert scope.named["sibling"] == frozenset({"pkg/sub/sibling.py"})

    def test_a_parent_relative_import_binds_the_package_above(self, tmp_path, extractor):
        resolver, _ = resolved(write(tmp_path, RELATIVE_PACKAGE), extractor)
        scope = resolver.scopes["pkg/sub/user.py"]

        assert scope.named["thing"] == frozenset({"pkg/helper.py"})

    def test_a_reference_through_a_relative_import_resolves_imported(self, tmp_path, extractor):
        _, graph = resolved(write(tmp_path, RELATIVE_PACKAGE), extractor)
        edges = edges_from(graph, "py:pkg/sub/user.py::go", "thing")

        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]
        assert edges[0].target.node == "py:pkg/helper.py::thing"


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
            graph,
            "py:gamma.py::ask",
            "py:alpha.py::shared",
            edge_policy=resolve.PathEdgePolicy(include_ambiguous=True),
        )
        assert not excluded.found
        assert included.found
        assert [hop.arrival.node for hop in included.hops] == ["py:alpha.py::shared"]

    def test_unique_edges_are_excluded_by_default(self, repo):
        _, graph = repo
        excluded = resolve.shortest_path(
            graph,
            "py:stranger.py::stray",
            "py:core.py::only_once",
        )
        included = resolve.shortest_path(
            graph,
            "py:stranger.py::stray",
            "py:core.py::only_once",
            edge_policy=resolve.PathEdgePolicy(include_unique=True),
        )

        assert not excluded.found
        assert included.found
        assert included.hops[0].edge.tier is resolve.Tier.UNIQUE

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


class TestImportCoverage:
    """An empty cycle list has to be readable against how much resolved."""

    def test_an_import_that_names_no_file_is_counted(self, tmp_path, extractor):
        root = write(tmp_path, {"a.py": "import json\nimport b\n", "b.py": "x = 1\n"})
        _, graph = resolved(root, extractor)
        assert graph.unresolved_imports == 1

    def test_a_repository_whose_imports_all_resolve_counts_none(self, tmp_path, extractor):
        _, graph = resolved(write(tmp_path, TWO_CYCLE), extractor)
        assert graph.unresolved_imports == 0

    def test_the_import_only_graph_reports_the_same_coverage(self, tmp_path, extractor):
        root = write(tmp_path, {"a.py": "import json\nimport b\n", "b.py": "x = 1\n"})
        scan = refs.scan_repo(root, extractor)
        assert resolve.import_graph(scan.files).unresolved_imports == 1

    def test_a_standard_library_import_is_not_counted_as_internal(self, tmp_path, extractor):
        """The count a caller reads coverage from must be zero when nothing was missed.

        `import json` names no file here and never could, so counting it makes
        a fully resolved repository report hundreds of failures: 508 of 1238
        statements on this repository, against 6 that named something it
        actually holds. `unresolved_imports` keeps its literal meaning and
        `unresolved_internal_imports` answers the coverage question.
        """
        root = write(tmp_path, {"a.py": "import json\nimport os\n", "b.py": "x = 1\n"})
        _, graph = resolved(root, extractor)

        assert graph.unresolved_imports == 2
        assert graph.unresolved_internal_imports == 0

    def test_an_import_naming_this_repository_is_counted_as_internal(self, tmp_path, extractor):
        # `pkg` is a directory here, so `pkg.missing` is a resolution this
        # package owed an answer to and did not give.
        root = write(
            tmp_path,
            {"pkg/__init__.py": "", "pkg/real.py": "x = 1\n", "a.py": "import pkg.missing\n"},
        )
        _, graph = resolved(root, extractor)

        assert graph.unresolved_internal_imports == 1

    def test_a_relative_import_that_resolves_to_nothing_is_always_internal(
        self, tmp_path, extractor
    ):
        # A relative import is spelled against the importing file's own
        # directory and can name nothing else, so it needs no segment table.
        root = write(tmp_path, {"pkg/__init__.py": "", "pkg/a.py": "from .missing import x\n"})
        _, graph = resolved(root, extractor)

        assert graph.unresolved_internal_imports == 1

    def test_the_import_only_graph_reports_both_counts(self, tmp_path, extractor):
        root = write(tmp_path, {"a.py": "import json\nimport b\n", "b.py": "x = 1\n"})
        scan = refs.scan_repo(root, extractor)
        built = resolve.import_graph(scan.files)

        assert (built.unresolved_imports, built.unresolved_internal_imports) == (1, 0)


# Two forms that genuinely do bring every name in unqualified, and one that
# looks like them and does not.
WHOLESALE_FILES = {
    "core.py": "def helper(value):\n    return value\n",
    "starred.py": "from core import *\n\n\ndef use(value):\n    return helper(value)\n",
    "module_only.py": "import core\n\n\ndef use(value):\n    return helper(value)\n",
}

# A `static inline` definition rather than a prototype: `_extract_c_symbols`
# reads `function_definition` and a prototype is a `declaration`, so a header
# of prototypes yields no symbols to resolve to. Recorded as B05-H8.
C_FILES = {
    "money.h": (
        "static inline double apply_tax(double amount, double rate) {\n"
        "    return amount * (1 + rate);\n}\n"
    ),
    "app.c": '#include "money.h"\n\ndouble total(double a) {\n    return apply_tax(a, 0.2);\n}\n',
}


class TestWholesaleEvidence:
    """Which imports may supply bare-name evidence, and which may not.

    The whole-module set used to hold the target of every module import, so
    "this file imported some module that happens to define this spelling" and
    "this file imported this name" both answered `resolved-via-import`. It now
    holds only the two forms that bring every name in unqualified.
    """

    def test_a_star_import_binds_every_name_it_brings_in(self, tmp_path, extractor):
        _, graph = resolved(write(tmp_path, WHOLESALE_FILES), extractor)
        edges = edges_from(graph, "py:starred.py::use", "helper")

        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]

    def test_a_module_import_binds_no_bare_name(self, tmp_path, extractor):
        # `import core` binds the module object. A bare `helper(value)` in
        # that file is a NameError, so the strongest honest answer is that
        # the name is unique in the repository -- not that it was imported.
        _, graph = resolved(write(tmp_path, WHOLESALE_FILES), extractor)
        edges = edges_from(graph, "py:module_only.py::use", "helper")

        assert [edge.tier for edge in edges] == [resolve.Tier.UNIQUE]

    def test_a_c_include_binds_every_name_the_header_declares(self, tmp_path, extractor):
        # The other direction: `#include` is a textual paste, so the arm has
        # to stay for C or every cross-file C reference loses a tier.
        if "c" not in grammars.warmed_languages():
            pytest.skip("grammar for c is not in the local pack cache")
        _, graph = resolved(write(tmp_path, C_FILES), extractor)
        edges = edges_from(graph, "c:app.c::total", "apply_tax")

        assert [edge.tier for edge in edges] == [resolve.Tier.IMPORTED]


# The same decision in a language `tests/conftest.py` always warms, so a cold
# language pack cannot turn it into no decision. `uses.go` declares a method
# named `Helper` and names the type `Helper` twice on that one line, as the
# receiver and as the result; `spread.go` names it twice one line down.
SAME_LINE_GO = {
    "helper.go": "package main\n\ntype Helper struct{}\n",
    "uses.go": "package main\n\nfunc (h Helper) Helper() Helper { return h }\n",
    "spread.go": "package main\n\nfunc (h Helper) Widen() Helper {\n\treturn h\n}\n",
}

# A declaration the Go table does not model: `var handler = 3` yields no
# symbol, so the guard has nothing to key on and the declaration's own name is
# left to resolve as a reference.
UNMODELLED_DECLARATION_GO = {
    "a.go": "package main\n\nfunc handler(x int) int { return x }\n",
    "b.go": "package main\n\nvar handler = 3\n",
}

# `Uses.Helper` names its return type, itself, and a constructor call, all on
# one line. The method name carries a declaration role, so the other two
# occurrences exercise the per-name/per-line proof in `_reference_edges`.
SAME_LINE_FILES = {
    "Helper.java": "public class Helper {\n    public int value() { return 1; }\n}\n",
    "Uses.java": ("public class Uses {\n    public Helper Helper() { return new Helper(); }\n}\n"),
}


class TestTheDeclarationGuardCostsBothWays:
    """What the declaration role recovers, and where the fallback remains.

    `_reference_edges` keeps its line-keyed proxy unless the extractor marked a
    declaration with the exact same name and line. That proof recovers other
    occurrences beside modelled declarations without disabling the fallback
    for unmodelled forms or for data-format keys.

    In Go, which `tests/conftest.py` warms on every run. The Java pair below
    says the same thing in the language the cost was first measured in, and
    skips when that grammar is cold; this class is why that skip cannot leave
    the decision unrecorded.
    """

    def test_a_reference_on_the_declaration_line_now_resolves(self, tmp_path, extractor):
        """A declaration role recovers the real references beside it.

        The receiver type and the result type on `uses.go` line 3 both name
        `helper.go`'s `Helper`; the method's own name stays filtered.
        """
        _, graph = resolved(write(tmp_path, SAME_LINE_GO), extractor)
        edges = edges_from(graph, "go:uses.go::Helper.Helper", "Helper")

        assert [(edge.target.path, edge.tier) for edge in edges] == [
            ("helper.go", resolve.Tier.AMBIGUOUS)
        ]

    def test_the_same_reference_one_line_down_resolves(self, tmp_path, extractor):
        """The guard is keyed on the line, so the cost is exactly one line wide."""
        _, graph = resolved(write(tmp_path, SAME_LINE_GO), extractor)
        edges = edges_from(graph, "go:spread.go::Helper.Widen", "Helper")

        assert {edge.target.path for edge in edges} == {"helper.go", "uses.go"}

    def test_a_declaration_with_no_symbol_is_read_as_a_reference(self, tmp_path, extractor):
        """Direction two: a wrong edge kept, which the guard's comment omits.

        The guard keys on the symbols a file recorded, so a declaration form
        the language table does not model leaves it nothing to match. `b.go`
        declares `handler` and defines nothing of that name, so the resolver
        reads the declaration's own identifier as a bare reference and answers
        it with the unrelated function in `a.go` -- at `unique`, the tier that
        means only that the repository spells the name once.

        This is the direction the C++ regression below closed by making the
        declaration a symbol. Closing it in general means recording the
        declaration role per language in the extractor, which is what the
        guard's comment in `core/resolve.py` names as the real fix.
        """
        _, graph = resolved(write(tmp_path, UNMODELLED_DECLARATION_GO), extractor)
        edges = [edge for edge in graph.edges if edge.name == "handler"]

        assert [(edge.source.path, edge.target.path, edge.tier) for edge in edges] == [
            ("b.go", "a.go", resolve.Tier.UNIQUE)
        ]


class TestAnOutOfLineDefinitionIsNotAReference:
    """The C++ instance of the guard's miss direction, closed at the source.

    `int Logger::emit(int x)` used to yield no symbol at all -- the extractor
    tested `str.isidentifier()` on the declarator's text and `Logger::emit` is
    not one -- so the guard had nothing to key on and both `emit` identifiers
    in the file became references to an unrelated free function in another
    file, at `unique`. Two confidently wrong edges from one missing symbol.
    """

    def test_a_qualified_definition_does_not_reference_a_free_function(self, tmp_path, extractor):
        if "cpp" not in grammars.warmed_languages():
            pytest.skip("grammar for cpp is not in the local pack cache")
        files = {
            "util.cpp": "int emit(int x) { return x; }\n",
            "logger.cpp": (
                "class Logger { public: int emit(int x); };\n\n"
                "int Logger::emit(int x) {\n    return x + 1;\n}\n"
            ),
        }
        _, graph = resolved(write(tmp_path, files), extractor)
        strays = [
            edge for edge in graph.edges if edge.name == "emit" and edge.target.path == "util.cpp"
        ]

        assert strays == []


class TestADeclarationSharingALineWithAReference:
    """A modelled declaration no longer hides neighbouring references.

    The extractor marks only names beneath configured declaration node types;
    data-format keys never reach this path. The resolver then removes the
    line-keyed fallback only for that exact declaration key.
    """

    def test_the_reference_on_the_declaration_line_now_resolves(self, tmp_path, extractor):
        if "java" not in grammars.warmed_languages():
            pytest.skip("grammar for java is not in the local pack cache")
        _, graph = resolved(write(tmp_path, SAME_LINE_FILES), extractor)

        edges = edges_from(graph, "java:Uses.java::Uses.Helper", "Helper")
        assert [edge.target.path for edge in edges] == ["Helper.java"]

    def test_a_reference_on_any_other_line_still_resolves(self, tmp_path, extractor):
        """The guard is keyed on the line, so one line down is enough."""
        if "java" not in grammars.warmed_languages():
            pytest.skip("grammar for java is not in the local pack cache")
        moved = dict(SAME_LINE_FILES)
        moved["Uses.java"] = (
            "public class Uses {\n"
            "    public Object Helper() {\n"
            "        return new Helper();\n"
            "    }\n"
            "}\n"
        )
        _, graph = resolved(write(tmp_path, moved), extractor)

        edges = edges_from(graph, "java:Uses.java::Uses.Helper", "Helper")
        assert [edge.target.path for edge in edges] == ["Helper.java"]
