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

        def counting(shown, packing):
            grouped.append(shown)
            return original(shown, packing)

        monkeypatch.setattr(map_service, "AUTO_BUDGET_PROBE", 1)
        monkeypatch.setattr(map_service, "AUTO_BUDGET_CEILING", 0)
        monkeypatch.setattr(map_service, "_group", counting)

        result = maps.build(repo, MapRequest())

        assert result.budget == AUTO_BUDGET_MAX
        assert grouped[0] == 1


# A repository that puts a Go-style test and a Python-style test in front of
# the same ranking. `svc/parse_test.go` sits in its subject's package, so it
# earns no import edge and is scored on damped name-reference weight alone --
# and it exercises two of the ranked files. `tests/test_app.py` earns the
# 3.0 import edge and exercises one. Ranking by weight inside the depth band
# puts the Python one first by an order of magnitude; ranking by how many
# ranked files each covers puts the Go one where it belongs.
MIXED_LANGUAGES = {
    "svc/parse.go": "package svc\n\nfunc Parse(text string) string {\n\treturn text\n}\n",
    "svc/build.go": "package svc\n\nfunc Build(text string) string {\n\treturn text\n}\n",
    "svc/parse_test.go": (
        'package svc\n\nimport "testing"\n\n'
        'func TestBoth(t *testing.T) {\n\tParse("a")\n\tBuild("b")\n}\n'
    ),
    "app.py": "def run(job):\n    return job\n",
    "tests/test_app.py": "from app import run\n\n\ndef test_run():\n    assert run(1) == 1\n",
}

# The mirror image. `tests/test_wide.py` imports three ranked modules on three
# lines, minting three 3.0-weight edges, and exercises exactly one of them.
# Counting imported targets would score it three and hand it the section;
# counting the targets it actually references scores it one, level with the Go
# test that exercises its own subject for real.
IMPORT_FAN_OUT = {
    "alpha.py": "def alpha(job):\n    return job\n",
    "bravo.py": "def bravo(job):\n    return job\n",
    "chose.py": "def chose(job):\n    return job\n",
    "svc/scan.go": "package svc\n\nfunc Scan(text string) string {\n\treturn text\n}\n",
    "tests/test_wide.py": (
        "from alpha import alpha\nfrom bravo import bravo\nfrom chose import chose\n\n\n"
        "def test_alpha_only():\n    assert alpha(1) == 1\n"
    ),
    "svc/scan_test.go": (
        'package svc\n\nimport "testing"\n\n'
        'func TestScan(t *testing.T) {\n\tif Scan("a") != "a" {\n\t\tt.Fatal("no")\n\t}\n}\n'
    ),
}


# One test file whose two functions reach one target each. Every other fixture
# here has a single referencing symbol per test, so this is the only shape that
# separates "what the file reaches" from "what the reported span reaches".
TWO_SPANS = {
    "alpha.py": "def alpha(job):\n    return job\n",
    "bravo.py": "def bravo(job):\n    return job\n",
    "tests/test_split.py": (
        "from alpha import alpha\nfrom bravo import bravo\n\n\n"
        "def test_alpha():\n    assert alpha(1) == 1\n\n\n"
        "def test_bravo():\n    assert bravo(2) == 2\n"
    ),
}


def written(tmp_path, files):
    """Write a repository out and resolve it."""
    for relative, text in files.items():
        target = tmp_path / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")
    return resolve_repo(tmp_path, None)


def companion(result, path):
    """The one companion row for ``path``, or ``None``."""
    return next((row for row in result.test_companions.rows if row.path == path), None)


class TestTheCompanionRankingIsNotWeightFirst:
    """The ordering rule, pinned against the bias it exists to remove.

    A test file is a pure source in the reference graph, so the ranking that
    scores inbound weight can never place one. The companion section is the
    only route a test has into a map, and the order it lists them in decides
    which tests an agent sees. Ordering by aggregated edge weight looks
    obvious and is a language filter: a Go test in its subject's package
    cannot earn an import edge, so weight-first sorts by language rather than
    by relevance.
    """

    def test_the_go_test_outranks_the_python_one_it_is_outweighed_by(self, tmp_path, maps):
        repo = written(tmp_path, MIXED_LANGUAGES)

        result = maps.build(repo, MapRequest(max_files=3))

        listed = [row.path for row in result.test_companions.rows]
        assert listed == ["svc/parse_test.go", "tests/test_app.py"]

    def test_the_order_is_not_the_one_weight_would_have_produced(self, tmp_path, maps):
        """The other half: the loser really does carry the larger weight.

        Without this the assertion above passes on a repository where the two
        weights happen to agree with the coverage counts, which would pin the
        ordering rule against nothing at all.
        """
        repo = written(tmp_path, MIXED_LANGUAGES)

        result = maps.build(repo, MapRequest(max_files=3))

        assert (
            companion(result, "tests/test_app.py").weight
            > companion(result, "svc/parse_test.go").weight
        )

    def test_the_go_test_wins_on_the_files_it_covers(self, tmp_path, maps):
        """Name the key that did the work, so a later change cannot silently swap it."""
        repo = written(tmp_path, MIXED_LANGUAGES)

        result = maps.build(repo, MapRequest(max_files=3))

        assert companion(result, "svc/parse_test.go").covers == ("svc/build.go", "svc/parse.go")
        assert companion(result, "tests/test_app.py").covers == ("app.py",)


class TestImportFanOutBuysNoCoverage:
    """The failure mode the coverage key can reintroduce by another route.

    Counting covered files fixes the weight bias and opens a second one: one
    ``from alpha import alpha, bravo, chose`` line mints an edge to three
    files whether or not the test touches any of them, so counting imported
    targets hands the section to whichever test imports the most. The count is
    therefore over the targets the file *references by name*, which is the
    same evidence the span is derived from and the only evidence that has a
    line to point at.
    """

    def test_a_test_covers_what_it_references_not_what_it_imports(self, tmp_path, maps):
        repo = written(tmp_path, IMPORT_FAN_OUT)

        result = maps.build(repo, MapRequest(max_files=4))

        assert companion(result, "tests/test_wide.py").covers == ("alpha.py",)

    def test_the_three_import_edges_are_still_there_to_be_counted(self, tmp_path, maps):
        """The edges exist; the rule declines to read them as coverage.

        Asserting the weight is what separates "fan-out was ignored" from
        "the imports never resolved", and only the first is the rule working.
        """
        repo = written(tmp_path, IMPORT_FAN_OUT)

        result = maps.build(repo, MapRequest(max_files=4))

        assert companion(result, "tests/test_wide.py").weight > 9.0

    def test_the_go_test_is_level_with_it_rather_than_buried_under_it(self, tmp_path, maps):
        """One covered file each, so the residual weight tiebreak decides.

        Recorded rather than corrected. Coverage is the key that has to be
        language-neutral; once two files genuinely cover the same number of
        ranked files, the plan's own reading is that the weight bias between
        them is far less consequential -- and a rule that also overturned the
        tiebreak would be pinned against nothing measurable.
        """
        repo = written(tmp_path, IMPORT_FAN_OUT)

        result = maps.build(repo, MapRequest(max_files=4))

        counts = {row.path: len(row.covers) for row in result.test_companions.rows}
        assert counts == {"tests/test_wide.py": 1, "svc/scan_test.go": 1}


class TestTheSectionIsAbsentRatherThanEmpty:
    """Zero tokens on a repository whose map reaches no test.

    Most repositories a map is asked about are in this state, and a heading
    over no rows spends the caller's budget saying the section found nothing
    -- which its absence already says. It is also what keeps the existing text
    goldens byte-identical, so the feature can be measured against them.
    """

    def test_a_repository_with_no_tests_lists_no_companions(self, repo, maps):
        result = maps.build(repo, MapRequest())

        assert result.test_companions.rows == ()
        assert result.test_companions.total == 0

    def test_the_render_says_nothing_about_a_section_it_has_no_rows_for(self, repo, maps):
        result = maps.build(repo, MapRequest())

        assert "tests exercising" not in maps.render_text(result)

    def test_a_test_that_only_imports_a_ranked_file_is_not_listed(self, tmp_path, maps):
        """An import is not a use, and it has no line to point at.

        The row's whole value over a bare path is the span, and an import-only
        edge says a module was pulled in without saying where anything from it
        is used. Listing it would mean fabricating a range or emitting the
        whole file, and the budgeted metrics this section exists for read a
        whole-file span as no answer at all.
        """
        repo = written(
            tmp_path,
            {
                "app.py": "def run(job):\n    return job\n",
                "other.py": "def other(job):\n    return job\n",
                "tests/test_unused.py": "import app\n\n\ndef test_nothing():\n    assert True\n",
            },
        )

        result = maps.build(repo, MapRequest(max_files=2))

        assert result.test_companions.rows == ()


class TestTheCapReportsWhatItCut:
    """A bounded section that cannot say what it left out reads as complete."""

    @pytest.fixture
    def crowded(self, tmp_path):
        """One ranked module and seven test files that all exercise it."""
        files = {"app.py": "def run(job):\n    return job\n"}
        for index in range(7):
            files[f"tests/test_{index}.py"] = (
                "from app import run\n\n\n"
                f"def test_{index}():\n    assert run({index}) == {index}\n"
            )
        return written(tmp_path, files)

    def test_the_cap_keeps_the_declared_number_of_rows(self, crowded, maps):
        result = maps.build(crowded, MapRequest(max_files=1))

        assert len(result.test_companions.rows) == map_service.DEFAULT_MAX_TEST_FILES

    def test_the_total_counts_every_test_the_walk_found(self, crowded, maps):
        result = maps.build(crowded, MapRequest(max_files=1))

        assert result.test_companions.total == 7

    def test_the_omitted_count_is_the_difference_and_is_stated(self, crowded, maps):
        result = maps.build(crowded, MapRequest(max_files=1))

        assert result.test_companions.omitted == 2
        assert "... 2 more test files not listed (limit 5)" in maps.render_text(result)


class TestEveryRowCarriesARealSpan:
    """A bare path costs the caller a read; a whole-file span costs the score.

    The answer contract these rows feed is ``path:start-end`` and it is scored
    on lines, so a row has to name the lines. The evaluator behind it expands
    a missing end to the whole file, which is exactly the artifact the
    budgeted metrics neutralize -- so a whole-file span is a row that moves
    nothing.
    """

    @pytest.fixture
    def spanned(self, tmp_path):
        """A test file whose first and last symbols both reference the subject."""
        return written(
            tmp_path,
            {
                "app.py": "def run(job):\n    return job\n\n\ndef pay(job):\n    return job\n",
                "tests/test_app.py": (
                    '"""A docstring, so line one is not a reference."""\n\n'
                    "from app import pay, run\n\n\n"
                    "def test_run():\n    assert run(1) == 1\n\n\n"
                    "def test_pay_and_run():\n"
                    "    assert pay(2) == 2\n    assert run(2) == 2\n"
                ),
            },
        )

    def test_the_span_is_the_referencing_symbol_not_the_whole_file(self, spanned, maps):
        result = maps.build(spanned, MapRequest(max_files=1))
        row = companion(result, "tests/test_app.py")

        assert (row.start, row.end) == (10, 12)

    def test_the_span_starts_after_the_first_line_and_ends_no_later_than_the_last(
        self, spanned, maps
    ):
        """The general form of the assertion above, stated as the invariant.

        The exact span pins this fixture; this pins the property every row has
        to hold, so a fixture whose symbols move still fails for the right
        reason.
        """
        result = maps.build(spanned, MapRequest(max_files=1))
        row = companion(result, "tests/test_app.py")
        lines = len((spanned.root / row.path).read_text(encoding="utf-8").splitlines())

        assert 1 <= row.start <= row.end
        assert (row.start, row.end) != (1, lines)

    def test_the_render_spells_the_span_the_way_the_contract_wants_it(self, spanned, maps):
        result = maps.build(spanned, MapRequest(max_files=1))

        assert "  tests/test_app.py:10-12  -- file references app.py" in maps.render_text(result)


class TestASeededFileCountsAsATarget:
    """`--focus` names the file the caller asked about, ranked or not.

    Seeds take the whole teleport mass, so a seed usually ranks -- but
    ``--max-files 1`` on a repository with two seeds keeps one of them, and
    the test that exercises the other is still the answer to the question the
    caller asked.
    """

    def test_a_test_of_an_unranked_seed_is_still_a_companion(self, tmp_path, maps):
        repo = written(
            tmp_path,
            {
                "alpha.py": "def alpha(job):\n    return job\n",
                "bravo.py": "def bravo(job):\n    return job\n",
                "tests/test_bravo.py": (
                    "from bravo import bravo\n\n\ndef test_bravo():\n    assert bravo(1) == 1\n"
                ),
            },
        )

        result = maps.build(repo, MapRequest(focus=("alpha.py", "bravo.py"), max_files=1))

        assert [map_file.path for map_file in result.files] == ["alpha.py"]
        assert companion(result, "tests/test_bravo.py").covers == ("bravo.py",)


class TestTheCompanionsTravelInBothForms:
    """The JSON is a shipped shape, and the file view is a view of the same map."""

    def test_the_json_carries_the_rows_the_total_and_the_omitted_count(self, tmp_path, maps):
        repo = written(tmp_path, MIXED_LANGUAGES)

        listed = maps.build(repo, MapRequest(max_files=3)).as_dict()["test_companions"]

        assert listed["total"] == 2
        assert listed["omitted"] == 0
        assert [row["path"] for row in listed["rows"]] == [
            "svc/parse_test.go",
            "tests/test_app.py",
        ]
        assert listed["rows"][0]["covers"] == ["svc/build.go", "svc/parse.go"]

    def test_the_file_granularity_answers_with_the_same_companions(self, tmp_path, maps):
        """Two granularities, one map: the section is not a symbol-view feature."""
        repo = written(tmp_path, MIXED_LANGUAGES)

        symbols = maps.build(repo, MapRequest(max_files=3))
        files = maps.build(repo, MapRequest(max_files=3, granularity=GRANULARITY_FILE))

        assert files.test_companions == symbols.test_companions


class TestTheCompanionWalkReportsItsOwnBound:
    """A walk that stopped looking must not read as a walk that found nothing.

    The section is bounded twice and the two cuts are different facts. The row
    cap trims after every test is found, so what it drops is counted. The
    flood's node bound stops the search itself, so what it drops was never
    seen and the total is a floor.
    """

    def test_a_finished_walk_reports_no_exhaustion(self, tmp_path, maps):
        repo = written(tmp_path, MIXED_LANGUAGES)

        result = maps.build(repo, MapRequest(max_files=3))

        assert result.test_companions.exhausted is False

    def test_a_capped_walk_says_so_in_the_listing(self, tmp_path, maps, monkeypatch):
        """The bound is the flood's, so it is forced there rather than faked
        on the listing: this pins the wiring, not the dataclass default.
        """
        real = map_service.flood
        monkeypatch.setattr(
            map_service,
            "flood",
            lambda *args, **kwargs: dataclasses.replace(real(*args, **kwargs), exhausted=True),
        )
        repo = written(tmp_path, MIXED_LANGUAGES)

        result = maps.build(repo, MapRequest(max_files=3))

        assert result.test_companions.exhausted is True
        assert "node bound" in maps.render_text(result)

    def test_the_json_carries_the_flag(self, tmp_path, maps):
        repo = written(tmp_path, MIXED_LANGUAGES)

        listed = maps.build(repo, MapRequest(max_files=3)).as_dict()["test_companions"]

        assert listed["exhausted"] is False


class TestCoversIsMeasuredOverTheWholeFile:
    """A guard on a contract nothing else pins.

    ``covers`` is the whole file's reach while the span is one symbol's, and
    the two are deliberately different extents. Narrowing ``covers`` to the
    chosen span would also change the ranking key in `companions_for`, which
    sorts on how many targets a row covers -- so a later "tidy-up" that made
    the two agree would silently reorder the section. Every existing fixture
    has one referencing symbol per test file, so nothing else would catch it.
    """

    def test_a_file_whose_two_functions_split_the_targets_covers_both(self, tmp_path, maps):
        repo = written(tmp_path, TWO_SPANS)

        result = maps.build(repo, MapRequest(max_files=2))
        row = companion(result, "tests/test_split.py")

        assert row is not None
        assert row.covers == ("alpha.py", "bravo.py")

    def test_the_span_is_one_function_not_the_hull_of_both(self, tmp_path, maps):
        """Which is exactly why the render must not read the span as the
        evidence for every name in ``covers``.
        """
        repo = written(tmp_path, TWO_SPANS)

        row = companion(maps.build(repo, MapRequest(max_files=2)), "tests/test_split.py")

        assert row is not None
        assert row.end - row.start < 4
