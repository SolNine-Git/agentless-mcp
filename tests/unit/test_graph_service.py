"""The graph service: explanations, paths and cycles as an agent sees them.

The core module owns whether an edge is right; what is asserted here is the
shape of the answer built out of those edges -- that a card carries its
definition site, that both fan sections are grouped strongest tier first, that
every bounded section says what it left out, and that the three ways a path
can fail to be a path are three different messages rather than an exception.
"""

import pytest

from agentless_mcp.application import render
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.repo_context import resolve_repo

CORE = """\
def helper(value):
    return value


def caller(value):
    return helper(value)
"""

USER = """\
from core import helper


def use(value):
    return helper(value)
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

ISLAND = """\
def marooned(argument):
    return argument
"""

FILES = {
    "core.py": CORE,
    "user.py": USER,
    "alpha.py": ALPHA,
    "beta.py": BETA,
    "gamma.py": GAMMA,
    "island.py": ISLAND,
}

CYCLE_FILES = {
    "x.py": "import y\n\n\ndef in_x():\n    return 1\n",
    "y.py": "import x\n\n\ndef in_y():\n    return 2\n",
}


def build(tmp_path, files):
    """Write a fixture repository and resolve its context."""
    for relative, text in files.items():
        (tmp_path / relative).write_text(text, encoding="utf-8")
    return resolve_repo(tmp_path, None)


@pytest.fixture
def repo(tmp_path):
    """The fixture repository every test in this module reads."""
    return build(tmp_path, FILES)


@pytest.fixture
def graphs(extractor):
    """The service under test."""
    return GraphService(extractor)


def tiers(groups):
    """The tier of each group, in the order they were rendered."""
    return [group.tier for group in groups]


class TestExplain:
    def test_the_card_names_the_definition_site(self, graphs, repo):
        explained = graphs.explain(repo, "helper")
        assert explained.card is not None
        assert explained.card.stable_id == "py:core.py::helper"
        assert explained.card.path == "core.py"
        assert explained.card.start_line == 1

    def test_fan_in_is_grouped_strongest_tier_first(self, graphs, repo):
        explained = graphs.explain(repo, "helper")
        assert tiers(explained.fan_in) == ["same_file", "imported"]
        first = explained.fan_in[0].rows[0]
        assert first.node == "py:core.py::caller"
        assert first.relation == "referenced by"

    def test_fan_out_carries_the_symbols_a_body_reaches(self, graphs, repo):
        explained = graphs.explain(repo, "caller")
        rows = [row.node for group in explained.fan_out for row in group.rows]
        assert "py:core.py::helper" in rows

    def test_the_import_relationships_of_the_file_are_reported(self, graphs, repo):
        explained = graphs.explain(repo, "helper")
        assert [row.other for row in explained.imports_in] == ["core.py"]
        assert [row.path for row in explained.imports_in] == ["user.py"]
        assert explained.imports_out == ()

    def test_an_unknown_target_is_a_message_not_an_exception(self, graphs, repo):
        explained = graphs.explain(repo, "no_such_symbol")
        assert explained.card is None
        assert "no_such_symbol" in explained.message
        assert render.render_explanation(explained).strip() == explained.message

    def test_a_colliding_name_explains_one_and_names_the_others(self, graphs, repo):
        explained = graphs.explain(repo, "shared")
        assert explained.card is not None
        assert explained.card.stable_id == "py:alpha.py::shared"
        assert explained.alternatives == ("py:beta.py::shared",)

    def test_a_section_limit_is_reported_rather_than_silent(self, graphs, repo):
        explained = graphs.explain(repo, "helper", limit=1)
        group = explained.fan_in[0]
        assert len(group.rows) == 1
        assert group.total >= 1
        rendered = render.render_explanation(explained)
        assert "more at this tier" in rendered or group.omitted == 0


class TestPath:
    def test_a_connected_pair_renders_hop_by_hop(self, graphs, repo):
        trace = graphs.path(repo, "use", "helper")
        assert trace.found
        assert trace.endpoints_resolved
        assert [hop.node for hop in trace.hops] == ["py:core.py::helper"]
        assert trace.hops[0].arrow == "->"
        assert trace.hops[0].tier == "imported"

    def test_no_path_is_an_answer_with_a_message(self, graphs, repo):
        trace = graphs.path(repo, "marooned", "helper")
        assert trace.endpoints_resolved
        assert not trace.found
        assert "no path" in trace.message
        assert "name-only-ambiguous" in trace.message

    def test_an_unknown_endpoint_is_named_not_raised(self, graphs, repo):
        trace = graphs.path(repo, "helper", "no_such_symbol")
        assert not trace.endpoints_resolved
        assert "no symbol or file matches no_such_symbol" in trace.message

    def test_an_ambiguous_endpoint_lists_the_candidates(self, graphs, repo):
        trace = graphs.path(repo, "ask", "shared")
        assert not trace.endpoints_resolved
        assert "py:alpha.py::shared" in trace.message
        assert "py:beta.py::shared" in trace.message

    def test_ambiguous_edges_join_the_search_only_when_asked(self, graphs, repo):
        excluded = graphs.path(repo, "ask", "py:alpha.py::shared")
        included = graphs.path(repo, "ask", "py:alpha.py::shared", include_ambiguous=True)
        assert not excluded.found
        assert included.found
        assert included.hops[0].tier == "ambiguous"

    def test_a_file_is_a_usable_endpoint(self, graphs, repo):
        trace = graphs.path(repo, "user.py", "core.py")
        assert trace.found
        assert trace.hops[0].node == "core.py"

    def test_the_search_bound_names_itself(self, graphs, repo):
        trace = graphs.path(repo, "use", "caller", max_visited=1)
        assert not trace.found
        assert trace.exhausted
        assert "search bound" in trace.message


class TestCycles:
    def test_a_repository_without_cycles_answers_plainly(self, graphs, repo):
        report = graphs.cycles(repo)
        assert report.total == 0
        assert render.render_cycles(report) == "no import cycles\n"

    def test_a_cycle_is_rendered_as_a_chain(self, graphs, tmp_path):
        report = graphs.cycles(build(tmp_path, CYCLE_FILES))
        assert report.total == 1
        assert report.cycles[0].chain == "x.py -> y.py -> x.py"
        assert "x.py -> y.py -> x.py" in render.render_cycles(report)

    def test_the_limit_caps_the_listing_and_says_so(self, graphs, tmp_path):
        files = {
            **CYCLE_FILES,
            "p.py": "import q\n\n\ndef in_p():\n    return 1\n",
            "q.py": "import r\n\n\ndef in_q():\n    return 2\n",
            "r.py": "import p\n\n\ndef in_r():\n    return 3\n",
        }
        report = graphs.cycles(build(tmp_path, files), limit=1)
        assert report.total == 2
        assert report.omitted == 1
        assert "1 more cycles not listed" in render.render_cycles(report)
