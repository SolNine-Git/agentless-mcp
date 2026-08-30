"""What an expansion does with an id it cannot honour, and what a fan-in says.

The cards themselves are covered in ``test_services``. What is pinned here is
the accounting around them: an id whose language prefix names another file's
language, an id sent twice, and the ids one call has no room for -- each of
which used to be answered by quietly doing something else.
"""

import json

import pytest

from agentless_mcp.application import envelope, render
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.symbol_service import (
    EXPAND_MAX_SEATS,
    MAX_UNRESOLVED_ROWS,
    SymbolService,
    is_fixture_path,
    is_test_path,
    render_expansion,
    render_refs,
    unresolved_lines,
)
from agentless_mcp.util.bounds import MAX_LIMIT
from agentless_mcp.util.errors import AgentlessError
from agentless_mcp.util.tokens import Chars4Counter

CORE = "def quote(value):\n    return value\n\n\ndef normalise(value):\n    return value\n"


@pytest.fixture
def repo(tmp_path):
    """A repository with two small symbols and one with more than a call can seat."""
    (tmp_path / "core.py").write_text(CORE, encoding="utf-8")
    (tmp_path / "wide.py").write_text(
        "".join(f"def step_{number}():\n    return {number}\n\n\n" for number in range(60)),
        encoding="utf-8",
    )
    return resolve_repo(tmp_path, None)


@pytest.fixture
def symbols(extractor, counter):
    """The service under test."""
    return SymbolService(extractor, counter)


def wide_ids(count):
    """Return ``count`` stable ids from the wide fixture file."""
    return [f"py:wide.py::step_{number}" for number in range(count)]


class TestTheLanguagePrefix:
    """An id names a language, and the resolver is held to it."""

    def test_a_prefix_that_names_another_language_is_refused(self, repo, symbols):
        """Reinterpreting the request was the failure, not refusing it.

        ``rs:core.py::quote`` answered with the Python symbol and printed
        ``py:core.py::quote`` on the card, so an agent holding a stale or
        hand-built id saw its own request corrected and could not tell.
        """
        result = symbols.expand_symbols(repo, ["rs:core.py::quote"])

        assert result.cards == ()
        entry, reason = result.unresolved[0]
        assert entry == "rs:core.py::quote"
        assert "the file is python" in reason
        assert "py:core.py::quote" in reason

    def test_the_matching_prefix_still_expands(self, repo, symbols):
        result = symbols.expand_symbols(repo, ["py:core.py::quote"])

        assert [card.stable_id for card in result.cards] == ["py:core.py::quote"]


class TestRepeatedIds:
    """A repeated id is one symbol, and it costs one seat."""

    def test_five_copies_expand_once_and_are_counted(self, repo, symbols):
        result = symbols.expand_symbols(repo, ["py:core.py::quote"] * 5)

        assert [card.stable_id for card in result.cards] == ["py:core.py::quote"]
        assert result.unresolved == (
            ("(4 ids)", "not expanded: repeated in this batch, and expanded under the first copy"),
        )

    def test_a_repeat_does_not_spend_another_id_s_room(self, repo, symbols):
        """The dedupe happens before the per-call limit, not after it.

        Two copies of one id used to fill two of the two slots a limit of two
        allows, so the second symbol the caller asked for was reported as
        over the limit.
        """
        result = symbols.expand_symbols(
            repo, ["py:core.py::quote", "py:core.py::quote", "py:core.py::normalise"], limit=2
        )

        assert [card.stable_id for card in result.cards] == [
            "py:core.py::quote",
            "py:core.py::normalise",
        ]


class TestIdsWithNoRoom:
    """The ids a call cannot answer are a count, not a list."""

    def test_ids_past_the_per_call_limit_are_one_row(self, repo, symbols):
        result = symbols.expand_symbols(repo, wide_ids(50), limit=10)

        assert len(result.cards) == 10
        assert result.unresolved == (
            ("(40 ids)", "not expanded: the per-call limit is 10 symbols"),
        )

    def test_ids_past_the_seat_count_are_one_row(self, repo, symbols):
        result = symbols.expand_symbols(repo, wide_ids(60), limit=60)

        assert len(result.cards) == EXPAND_MAX_SEATS
        entry, reason = result.unresolved[0]
        assert entry == f"({60 - EXPAND_MAX_SEATS} ids)"
        assert reason.startswith("not expanded: 60 ids exceed the 40")

    def test_the_wrapped_json_keeps_every_card_it_seated(self, repo, symbols, counter):
        """The reason the rows are collapsed at all.

        The service budget governs the cards; the envelope's ceiling governs
        the whole document. One sentence per over-bound id was charged to the
        second and not covered by the first, so the ceiling dropped whole
        symbol cards -- the failure the fair split exists to prevent.
        """
        result = symbols.expand_symbols(repo, wide_ids(60), limit=60)
        document = json.loads(
            envelope.wrap_json(repo, result.as_dict(), counter=counter, items_key="symbols")
        )

        assert "truncated" not in document
        assert len(document["symbols"]) == EXPAND_MAX_SEATS


class TestBounds:
    """Every bound this call takes is checked, not only ``limit``."""

    @pytest.mark.parametrize(
        ("name", "value"),
        [("limit", 0), ("budget", 0), ("seats", 0), ("seats", -1)],
    )
    def test_a_bound_below_one_is_refused(self, repo, symbols, name, value):
        """``seats=-1`` sliced the card list from the end and kept all but one.

        It then reported the cards it had kept as the ones with no room,
        which is the negative-slicing defect the shared bound refuses.
        """
        with pytest.raises(AgentlessError, match=name):
            symbols.expand_symbols(repo, ["py:core.py::quote"], **{name: value})


class TestAnUnresolvedFanInTarget:
    """A fan-in that fell back to the name says so."""

    def test_an_id_naming_no_symbol_is_reported_as_unresolved(self, repo, symbols):
        """The fallback is deliberate; echoing the id back as ``target`` was not.

        The rows come back on the strongest evidence tier for a symbol the
        caller never named, and every other partial answer in this module is
        labelled.
        """
        result = symbols.find_referencing_symbols(repo, "py:no/such/file.py::quote")

        assert result.target_resolved is False
        assert result.as_dict()["target_resolved"] is False
        assert "resolves to no symbol" in result.notice
        assert "the name quote" in result.notice

    def test_an_id_that_resolves_carries_no_notice(self, repo, symbols):
        result = symbols.find_referencing_symbols(repo, "py:core.py::quote")

        assert result.target_resolved is True
        assert result.notice == ""

    def test_a_bare_name_was_never_a_promise_about_a_file(self, repo, symbols):
        result = symbols.find_referencing_symbols(repo, "quote")

        assert result.target_resolved is True
        assert result.notice == ""

    def test_the_text_render_carries_the_notice_above_the_rows(self, repo, symbols):
        """The notice reached the JSON form and nothing else.

        Both adapters built the text from the row renderer alone, so the
        reader who gets text was handed the strongest evidence tier for a
        symbol nobody named, with no sign the id had degraded to a name.
        """
        result = symbols.find_referencing_symbols(repo, "py:no/such/file.py::quote")
        rendered = render_refs(result)

        assert rendered.startswith(result.notice)
        assert "resolves to no symbol" in rendered.split("\n\n", 1)[0]

    def test_a_resolved_target_renders_the_rows_alone(self, repo, symbols):
        """No notice, no warning: the composed render is the row render.

        ``complete_over`` rides along because this fan-in is empty over a
        complete scan, and the absence claim belongs to both spellings of the
        same answer.
        """
        result = symbols.find_referencing_symbols(repo, "py:core.py::quote")

        assert render_refs(result) == render.render_ref_groups(
            result.groups, result.target, complete_over=result.scanned
        )

    def test_the_shared_caller_render_carries_the_notice_too(self, repo, symbols):
        result = symbols.find_referencing_symbols(
            repo, "py:no/such/file.py::quote", shared_callers=True
        )
        rendered = render_refs(result, shared_callers=True)

        assert rendered.startswith(result.notice)
        assert render.render_shared_callers(result.shared, result.target) in rendered


class TestAnEmptyFanInStatesItsEvidence:
    """Absence is a claim, and a claim states what licenses it.

    "no references" over a complete scan and the same words over a scan that
    skipped files are different answers, and only the first lets the caller
    skip a verification search. The refs door never carried the skipped-file
    warning ``render_find`` prepends, so a partial-scan empty fan-in read as
    affirmative absence.
    """

    def test_a_complete_scan_licenses_the_absence_claim(self, repo, symbols):
        result = symbols.find_referencing_symbols(repo, "normalise")
        rendered = render_refs(result)

        assert "no references to normalise" in rendered
        assert "(complete scan: 2 files, 0 skipped)" in rendered

    def test_a_partial_scan_warns_instead_of_claiming(self, tmp_path, extractor, counter):
        (tmp_path / "core.py").write_text(CORE, encoding="utf-8")
        (tmp_path / "huge.py").write_text("x = 1\n" * 200_000, encoding="utf-8")
        ctx = resolve_repo(tmp_path, None)

        result = SymbolService(extractor, counter).find_referencing_symbols(ctx, "normalise")
        rendered = render_refs(result)

        assert "// warning: 1 files were skipped" in rendered
        assert "complete scan" not in rendered

    def test_a_populated_fan_in_carries_no_claim(self, tmp_path, extractor, counter):
        (tmp_path / "core.py").write_text(CORE, encoding="utf-8")
        (tmp_path / "billing.py").write_text(
            "from core import quote\n\n\ndef run():\n    return quote(1)\n",
            encoding="utf-8",
        )
        ctx = resolve_repo(tmp_path, None)

        rendered = render_refs(
            SymbolService(extractor, counter).find_referencing_symbols(ctx, "quote")
        )

        assert "reference" in rendered
        assert "complete scan" not in rendered

    def test_the_json_carries_the_scan_evidence(self, repo, symbols):
        document = symbols.find_referencing_symbols(repo, "normalise").as_dict()

        assert document["scanned"] == 2
        assert document["skipped"] == []


class TestTheUnresolvedListIsBounded:
    """A failure report must not be able to crowd out the answer.

    Every "no longer defines X" reason embeds its own file and symbol, so a
    batch of bogus ids produces a batch of *distinct* reasons and the existing
    group-by-identical-reason path collapses none of them. Measured before the
    bound: 40 real ids beside 460 bogus ones at ``limit=500`` returned 460
    unresolved rows and 15.9k JSON tokens, which pushed the envelope ceiling
    down onto the cards and returned 10 of the 40 bodies actually asked for.
    """

    def test_five_hundred_bogus_ids_do_not_produce_five_hundred_rows(self, repo, symbols):
        bogus = [f"py:wide.py::absent_{number}" for number in range(500)]

        result = symbols.expand_symbols(repo, bogus, limit=MAX_LIMIT)

        assert len(result.unresolved) == MAX_UNRESOLVED_ROWS
        assert result.unresolved_omitted == 500 - MAX_UNRESOLVED_ROWS
        assert result.unresolved_total == 500

    def test_the_ids_it_did_not_name_are_counted_not_dropped(self, repo, symbols):
        bogus = [f"py:wide.py::absent_{number}" for number in range(500)]

        lines = unresolved_lines(symbols.expand_symbols(repo, bogus, limit=MAX_LIMIT))

        assert "500 of the ids requested did not resolve" in lines[-1]

    def test_the_bound_leaves_room_for_the_cards_that_did_resolve(self, repo, symbols):
        """The regression this bound exists for: rows crowding out bodies."""
        requested = [*wide_ids(40), *[f"py:wide.py::absent_{n}" for n in range(460)]]

        result = symbols.expand_symbols(repo, requested, limit=MAX_LIMIT)
        wrapped = envelope.wrap_json(repo, result.as_dict(), counter=Chars4Counter())

        assert len(result.cards) == EXPAND_MAX_SEATS
        assert len(json.loads(wrapped)["symbols"]) == EXPAND_MAX_SEATS


class TestTheFailureReportIsNotPartOfTheAnswer:
    """`expand` was the unedited sibling of the `skeleton` defect.

    `skeleton a.py missing.py` used to render the read failure into stdout
    among the file contents at exit 0, and was fixed. `expand` did the same
    thing with ids that missed, and was not: an agent piping the output into a
    prompt read "unresolved: ... no longer defines X" as source among the
    sources it asked for.
    """

    def test_the_rendered_body_carries_only_the_answer(self, repo, symbols):
        result = symbols.expand_symbols(repo, ["py:core.py::quote", "py:core.py::absent"])

        assert "def quote" in render_expansion(result)
        assert "unresolved" not in render_expansion(result)

    def test_the_ids_that_missed_are_still_reported_separately(self, repo, symbols):
        result = symbols.expand_symbols(repo, ["py:core.py::quote", "py:core.py::absent"])

        assert any("absent" in line for line in unresolved_lines(result))


# One helper called from three production files and one test file. The
# production paths sort first, so a flat prefix cut spends the whole limit on
# them before the test file is reached.
MIXED_CALLER = """\
from src.core import widget


def use_{index}():
    first = widget()
    return first + widget()
"""

MIXED_TEST = """\
from src.core import widget


def test_widget():
    assert widget() == 1
"""

MIXED_PRODUCTION_FILES = ("alpha", "beta", "gamma")


@pytest.fixture
def mixed_fan_in(tmp_path):
    """A repository whose helper is called from `src/` files and one `tests/` file."""
    (tmp_path / "src").mkdir()
    (tmp_path / "tests").mkdir()
    (tmp_path / "src" / "core.py").write_text("def widget():\n    return 1\n", encoding="utf-8")
    for index, name in enumerate(MIXED_PRODUCTION_FILES):
        (tmp_path / "src" / f"{name}.py").write_text(
            MIXED_CALLER.format(index=index), encoding="utf-8"
        )
    (tmp_path / "tests" / "test_widget.py").write_text(MIXED_TEST, encoding="utf-8")
    return resolve_repo(tmp_path, None)


class TestTheLimitIsSpentOnBreadth:
    """A truncated fan-in used to drop the test files and say nothing.

    `_dedupe` orders the sites by `(path, line, name)` and the limit used to
    cut that list flat, so the tail it dropped was whichever files sort last
    -- and `src` sorts before `tests`. A symbol with more sites than the limit
    answered with its production callers alone, which an agent reads as "no
    test covers this". Nothing demoted the tests; the alphabet did.
    """

    def test_a_test_file_survives_a_limit_the_production_files_could_fill(
        self, mixed_fan_in, symbols
    ):
        result = symbols.find_referencing_symbols(mixed_fan_in, "widget", limit=4)

        assert any(group.path.startswith("tests/") for group in result.groups)

    def test_every_referencing_file_is_represented_before_any_gets_a_second_site(
        self, mixed_fan_in, symbols
    ):
        result = symbols.find_referencing_symbols(mixed_fan_in, "widget", limit=4)

        assert [len(group.sites) for group in result.groups] == [1, 1, 1, 1]

    def test_the_counts_still_speak_for_every_site_and_file(self, mixed_fan_in, symbols):
        """The round-robin decides what is shown, never what is counted."""
        result = symbols.find_referencing_symbols(mixed_fan_in, "widget", limit=4)
        whole = symbols.find_referencing_symbols(mixed_fan_in, "widget")

        assert result.groups.total == whole.groups.total
        assert result.groups.files == len(MIXED_PRODUCTION_FILES) + 1
        assert result.groups.files_omitted == 0
        assert result.groups.omitted == whole.groups.total - 4

    def test_a_limit_below_the_file_count_still_reports_the_files_it_cut(
        self, mixed_fan_in, symbols
    ):
        result = symbols.find_referencing_symbols(mixed_fan_in, "widget", limit=2)

        assert len(result.groups) == 2
        assert result.groups.files_omitted == 2

    def test_the_sites_of_one_file_keep_their_line_order(self, mixed_fan_in, symbols):
        result = symbols.find_referencing_symbols(mixed_fan_in, "widget")
        lines = [[site.line for site in group.sites] for group in result.groups]

        assert all(group == sorted(group) for group in lines)


class TestWhatCountsAsATestPath:
    """The predicate is what makes a test file visible as one.

    It was scoped to `test`/`tests` directory segments, which sees a Python
    repository and misses the languages that put the test beside the code:
    Go, Rust and TypeScript name the file, not the directory. Measured
    against the bench ground truth, the directory rule alone missed 11 of 57
    test files, and in-package suffix-named files were the dominant class.
    """

    @pytest.mark.parametrize(
        "path",
        [
            "tests/test_pricing.py",
            "conftest.py",
            "pkg/conftest.py",
            "testing/harness.go",
            "spec/models.rb",
            "specs/acceptance.rb",
            "__tests__/App.jsx",
            "config/os_test.go",
            "rpc/flipt/validation_test.go",
            "src/ledger_test.rs",
            "pkg/thing_test.py",
            "applications/web/shareUrl.test.ts",
            "app/models.spec.ts",
        ],
    )
    def test_each_convention_reads_as_a_test_file(self, path):
        assert is_test_path(path)

    @pytest.mark.parametrize(
        "path",
        [
            "latest/pricing.py",
            "contest.py",
            "introspection.py",
            "src/core.py",
            "spectrum/analyse.py",
        ],
    )
    def test_a_name_that_merely_contains_the_word_is_production(self, path):
        """A substring rule reads `spec` out of `introspection.py` and fails here."""
        assert not is_test_path(path)


class TestWhatCountsAsAFixturePath:
    """Beside the test predicate rather than folded into it.

    A fixture exists to be parsed, so nothing calls it and it is a permanent
    orphan candidate; a fixture is not a test, so it must not rank as one in
    the map's companion section. Two questions, two names, and the health
    view is the caller that asks both.
    """

    @pytest.mark.parametrize(
        "path",
        ["fixtures/repo_py/core.py", "tests/characterization/fixtures/sample.py"],
    )
    def test_a_fixture_directory_reads_as_one(self, path):
        assert is_fixture_path(path)

    @pytest.mark.parametrize("path", ["fixtures_old/core.py", "src/fixture.py", "src/core.py"])
    def test_a_name_that_merely_contains_the_word_is_production(self, path):
        assert not is_fixture_path(path)

    def test_a_fixture_is_not_reported_as_a_test(self):
        """The map lists tests as companions; a fixture has nothing to exercise."""
        assert not is_test_path("fixtures/repo_py/core.py")
