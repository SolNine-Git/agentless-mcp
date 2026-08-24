"""What the repository map lists, how it scores symbols, and what it bounds.

Companion to the map cases in ``test_services.py``. Those pin the ranking a
map produces; these pin what the view is allowed to leave out, what it has to
say when it leaves something out, and which knobs reach the symbol stage.
"""

import dataclasses

import pytest

from agentless_mcp.application import map_service
from agentless_mcp.application.map_service import (
    AUTO_BUDGET_MAX,
    GRANULARITY_FILE,
    MapRequest,
    MapService,
)
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import refs
from agentless_mcp.core.graph import build_graph, personalized_pagerank, rank_order
from agentless_mcp.core.projectconfig import MAX_BUDGET, MIN_BUDGET
from agentless_mcp.util.errors import OperationFailed

# `pkg/__init__.py` is the point of this tree: it is a real, ranked file that
# extracts no symbol. Python packages and TypeScript barrels are the usual
# shape, and they are exactly the files that name a package's public surface.
PACKAGE = {
    "pkg/__init__.py": "",
    "pkg/hub.py": "def run(job):\n    return job\n\n\ndef pay(job):\n    return job\n",
    "pkg/alpha.py": "from pkg.hub import run\n\n\ndef alpha():\n    return run(1)\n",
    "pkg/bravo.py": "from pkg.hub import run\n\n\ndef bravo():\n    return run(2)\n",
    "pkg/chose.py": "from pkg.hub import run\n\n\ndef chose():\n    return run(3)\n",
    "pkg/delta.py": "from pkg.hub import pay\n\n\ndef delta():\n    return pay(4)\n",
}


@pytest.fixture
def repo(tmp_path):
    """A package whose entry point defines nothing and still ranks."""
    for relative, text in PACKAGE.items():
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")
    return resolve_repo(tmp_path, None)


@pytest.fixture
def maps(extractor, counter):
    """The service under test, wired the way the composition root wires it."""
    return MapService(extractor, counter)


def listed(result):
    """The paths the map rendered, in the order it rendered them."""
    return [map_file.path for map_file in result.files]


class TestEveryRankedFileIsListed:
    """A file that placed no symbol is still a file the ranking chose.

    The file list used to be built from the *symbols* that survived, so a
    ranked file defining nothing vanished from the default granularity with
    no count anywhere saying a file had gone. An agent asking where a
    package's public surface lives was shown nothing and told nothing.
    """

    def test_a_ranked_file_with_no_symbols_still_gets_a_header(self, repo, maps):
        result = maps.build(repo, MapRequest())

        assert "pkg/__init__.py" in listed(result)

    def test_the_map_lists_as_many_files_as_the_ranking_kept(self, repo, maps):
        result = maps.build(repo, MapRequest())

        assert len(result.files) == result.ranked == len(PACKAGE)

    def test_the_json_form_carries_the_symbol_less_file_too(self, repo, maps):
        document = maps.build(repo, MapRequest()).as_dict()

        entry = next(row for row in document["files"] if row["path"] == "pkg/__init__.py")
        assert entry["symbols"] == []
        assert entry["omitted"] == 0

    def test_the_files_stay_in_rank_order(self, repo, maps):
        result = maps.build(repo, MapRequest())

        ranks = [map_file.rank for map_file in result.files]
        assert ranks == sorted(ranks, reverse=True)

    def test_the_file_view_and_the_symbol_view_list_the_same_files(self, repo, maps):
        symbols = maps.build(repo, MapRequest())
        files = maps.build(repo, MapRequest(granularity=GRANULARITY_FILE))

        assert listed(symbols) == listed(files)


class TestTheFileViewIsNotABudgetedView:
    """`file` granularity shows no symbols by design and has no budget.

    Reporting the repository's symbol count as "available" made both adapters
    render `0 of N symbols shown ... raise the budget` over a view with no
    budget to raise.
    """

    def test_no_symbol_competed_for_a_place(self, repo, maps):
        result = maps.build(repo, MapRequest(granularity=GRANULARITY_FILE))

        assert (result.included, result.candidates, result.budget) == (0, 0, 0)

    def test_each_file_still_carries_its_own_symbol_count(self, repo, maps):
        result = maps.build(repo, MapRequest(granularity=GRANULARITY_FILE))

        hub = next(row for row in result.files if row.path == "pkg/hub.py")
        assert hub.omitted == 2


class TestOmittedIsDerivedNotHandPassed:
    """One subtraction, in the one place that owns it.

    Both granularities used to pass `omitted` in as a difference they worked
    out themselves, against two different denominators, which is exactly what
    the `_Bounded` protocol was introduced to make impossible.
    """

    def test_the_file_view_counts_every_symbol_as_omitted(self, repo, maps):
        result = maps.build(repo, MapRequest(granularity=GRANULARITY_FILE))

        for row in result.files:
            assert row.shown == 0
            assert row.omitted == row.total

    def test_the_symbol_view_omits_what_it_did_not_list(self, repo, maps):
        result = maps.build(repo, MapRequest())

        for row in result.files:
            assert row.shown == len(row.entries)
            assert row.omitted == row.total - len(row.entries)
            assert row.as_dict()["omitted"] == row.omitted


class TestTheRankingSaysWhetherItSettled:
    """A partial ranking rendered as a finished one is the failure to avoid."""

    def test_a_settled_ranking_is_reported_as_one(self, repo, maps):
        result = maps.build(repo, MapRequest())

        assert result.rank_converged
        assert result.as_dict()["rank_converged"] is True

    def test_an_unsettled_ranking_is_named_above_the_map(self, repo, maps):
        result = dataclasses.replace(maps.build(repo, MapRequest()), rank_converged=False)

        assert result.as_dict()["rank_converged"] is False
        assert "did not converge" in maps.render_text(result)


class TestTheBudgetBoundsTheBodyNotTheHeaders:
    """The packing search can only drive symbols to zero, never headers.

    Every ranked file is listed whatever the budget says, so a small budget
    is honoured for the symbol bodies and exceeded by the render as a whole.
    Reported rather than hidden: a budget silently overrun reads as a budget
    that held.
    """

    @staticmethod
    @pytest.fixture
    def wide_repo(tmp_path, pinned_context):
        """Enough ranked files that their headers alone outrun the smallest budget.

        This used to pass ``budget=1``, which is no longer a budget any door
        accepts: ``projectconfig`` declares 200..64000, the MCP schema
        publishes it, and ``MapService`` now holds callers to it too. The
        behaviour under test is real -- headers are listed whatever the
        budget says -- so the fixture grows until the smallest *legal* budget
        reaches it, rather than the bound bending to the test.
        """
        for index in range(40):
            name = f"a_module_with_a_long_name_{index:03d}.py"
            (tmp_path / name).write_text(f"def f{index}():\n    return {index}\n", encoding="utf-8")
        return pinned_context(tmp_path)

    def test_a_budget_the_headers_alone_exceed_is_named_in_the_render(self, wide_repo, maps):
        result = maps.build(wide_repo, MapRequest(budget=MIN_BUDGET))

        assert result.included == 0
        assert result.rendered > result.budget
        assert f"renders to {result.rendered} tokens" in maps.render_text(result)

    def test_a_budget_the_render_fits_inside_says_nothing(self, repo, maps):
        result = maps.build(repo, MapRequest(budget=AUTO_BUDGET_MAX))

        assert result.rendered <= result.budget
        assert "renders to" not in maps.render_text(result)

    def test_the_rendered_cost_travels_in_the_json_too(self, wide_repo, maps):
        result = maps.build(wide_repo, MapRequest(budget=MIN_BUDGET))

        assert result.as_dict()["rendered_tokens"] == result.rendered

    def test_a_budget_below_the_published_floor_is_refused(self, repo, maps):
        """The floor projectconfig and the MCP schema already declared.

        `map --budget 1` used to exit 0 and report that the budget left room
        for no symbols -- a fact about the request, worded as one about the
        repository. The CLI was the only one of the three doors with no bound
        behind it.
        """
        with pytest.raises(OperationFailed, match="budget takes a value from 200 through 64000"):
            maps.build(repo, MapRequest(budget=1))

    def test_a_budget_above_the_published_ceiling_is_refused(self, repo, maps):
        with pytest.raises(OperationFailed, match="budget takes a value from 200 through 64000"):
            maps.build(repo, MapRequest(budget=MAX_BUDGET + 1))


class TestSymbolScoresReadTheSameDampingTheEdgesDo:
    """The inbound count is a count of bare-name sites, so it is damped.

    Called directly rather than through a budget: the packing search is a
    bisection over a rendered string, and on a tree small enough to reason
    about, the chars/4 estimator admits every symbol or none. The scores are
    the behaviour under test, so the scores are what the test reads.
    """

    @staticmethod
    def scored(repo, extractor, stoplist):
        scan = refs.scan_repo(repo.root, extractor)
        index = refs.build_ref_index(scan)
        rank = personalized_pagerank(build_graph(scan, index, stoplist=stoplist)).rank
        chosen = rank_order(rank)
        return [
            candidate.symbol.name
            for candidate in map_service._score_symbols(
                chosen, scan.by_path(), index, rank, stoplist
            )
        ]

    def test_a_stoplisted_name_stops_outranking_the_symbol_beside_it(self, repo, extractor):
        """``run`` is referenced three times and ``pay`` once, so it wins undamped."""
        plain = self.scored(repo, extractor, frozenset())
        stopped = self.scored(repo, extractor, frozenset({"run"}))

        assert plain.index("run") < plain.index("pay")
        assert stopped.index("pay") < stopped.index("run")

    def test_the_repositorys_stoplist_reaches_the_symbol_stage(self, repo, maps, extractor):
        """Through the service, not the helper: the knob has to be wired."""
        stopped = dataclasses.replace(
            repo, config=dataclasses.replace(repo.config, stoplist=frozenset({"run"}))
        )

        assert maps.build(stopped, MapRequest()).files
        assert self.scored(stopped, extractor, stopped.config.stoplist) != self.scored(
            repo, extractor, frozenset()
        )


class TestTheAutoBudgetStopsWhenTheAnswerIsSettled:
    """The estimate is clamped, so rendering past the ceiling cannot move it."""

    def test_a_stepped_probe_returns_what_a_whole_render_returned(self, repo, maps, monkeypatch):
        whole = maps.build(repo, MapRequest()).budget
        monkeypatch.setattr(map_service, "AUTO_BUDGET_PROBE", 1)

        assert maps.build(repo, MapRequest()).budget == whole

    def test_the_probe_stops_at_the_first_prefix_past_the_ceiling(self, repo, maps, monkeypatch):
        grouped: list[int] = []
        original = map_service._group

        def counting(included, candidates, paths, rank):
            grouped.append(len(included))
            return original(included, candidates, paths, rank)

        monkeypatch.setattr(map_service, "AUTO_BUDGET_PROBE", 1)
        monkeypatch.setattr(map_service, "AUTO_BUDGET_CEILING", 0)
        monkeypatch.setattr(map_service, "_group", counting)

        result = maps.build(repo, MapRequest())

        assert result.budget == AUTO_BUDGET_MAX
        assert grouped[0] == 1
