"""Map, view and symbol services against a small purpose-built repository."""

import pytest

from agentless_mcp.application import render
from agentless_mcp.application.map_service import (
    AUTO_BUDGET_MAX,
    AUTO_BUDGET_MIN,
    GRANULARITY_FILE,
    MapRequest,
    MapService,
    seed_weights,
)
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.symbol_service import SymbolService, render_find
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import grammars, refs
from agentless_mcp.core.projectconfig import MIN_BUDGET
from agentless_mcp.core.symbols import SIGNATURE_MAX_CHARS
from agentless_mcp.util.errors import AgentlessError, LanguageUnavailable
from agentless_mcp.util.tokens import Chars4Counter

CORE = '''\
"""Core."""

RATE = 3


def quote(sku):
    return RATE


def normalise(amount):
    return round(amount, 2)


class PriceBook:
    def cost_of(self, sku):
        return quote(sku)
'''

BILLING = """\
from core import normalise, quote


def run_billing(items):
    return sum(normalise(quote(item)) for item in items)


def audit(items):
    return len(items)
"""

LEDGER = """\
from core import normalise, quote


class Ledger:
    def post(self, item):
        return normalise(quote(item))

    def reconcile(self, item):
        return quote(item)
"""

ORPHAN = """\
def lonely():
    return 0
"""


@pytest.fixture
def repo(tmp_path):
    """A four-file repository whose reference graph has an obvious centre."""
    (tmp_path / "core.py").write_text(CORE, encoding="utf-8")
    (tmp_path / "billing.py").write_text(BILLING, encoding="utf-8")
    (tmp_path / "ledger.py").write_text(LEDGER, encoding="utf-8")
    (tmp_path / "orphan.py").write_text(ORPHAN, encoding="utf-8")
    return resolve_repo(tmp_path, None)


class TestMapService:
    def test_rationale_nodes_are_rendered_below_their_symbol(self, tmp_path, extractor, counter):
        (tmp_path / "planner.py").write_text(
            "def choose(value):\n"
            "    # WHY: stable ordering is part of the contract\n"
            "    return value\n",
            encoding="utf-8",
        )
        ctx = resolve_repo(tmp_path, None)
        maps = MapService(extractor, counter)

        result = maps.build(ctx, MapRequest(focus=("choose",)))
        text = maps.render_text(result)
        entry = result.files[0].entries[0]

        assert entry.rationales[0].parent_id == entry.stable_id
        assert entry.rationales[0].kind == "why"
        assert "# WHY: stable ordering is part of the contract" in text
        assert "::rationale@2 -> py:planner.py::choose" in text

    def test_the_most_referenced_file_ranks_first(self, repo, extractor, counter):
        result = MapService(extractor, counter).build(repo, MapRequest())
        assert result.files[0].path == "core.py"

    def test_focus_pulls_a_named_file_up_the_ranking(self, repo, extractor, counter):
        maps = MapService(extractor, counter)
        unfocused = maps.build(repo, MapRequest())
        focused = maps.build(repo, MapRequest(focus=("orphan.py",)))

        assert focused.seeds == ("orphan.py",)
        unfocused_rank = {entry.path: entry.rank for entry in unfocused.files}
        focused_rank = {entry.path: entry.rank for entry in focused.files}
        assert focused_rank["orphan.py"] > unfocused_rank["orphan.py"]

    def test_a_symbol_focus_seeds_the_file_that_defines_it(self, repo, extractor, counter):
        result = MapService(extractor, counter).build(repo, MapRequest(focus=("lonely",)))
        assert result.seeds == ("orphan.py",)

    def test_a_focus_matching_nothing_degrades_to_an_unfocused_map(self, repo, extractor, counter):
        maps = MapService(extractor, counter)
        typo = maps.build(repo, MapRequest(focus=("nosuchthing",)))
        assert typo.seeds == ()
        assert [f.path for f in typo.files] == [
            f.path for f in maps.build(repo, MapRequest()).files
        ]

    def test_a_method_name_seeds_the_file_that_defines_it(self, repo, extractor, counter):
        """The seed shape an issue report actually yields."""
        result = MapService(extractor, counter).build(repo, MapRequest(focus=("reconcile",)))
        assert result.seeds == ("ledger.py",)
        assert result.unresolved_seeds == ()
        assert result.files[0].path == "ledger.py"

    def test_a_module_stem_focuses_the_map_on_that_file(self, repo, extractor, counter):
        """`orphan` must seed orphan.py as if the caller had typed orphan.py."""
        result = MapService(extractor, counter).build(repo, MapRequest(focus=("orphan",)))
        assert result.seeds == ("orphan.py",)
        assert result.unresolved_seeds == ()
        assert result.files[0].path == "orphan.py"

    def test_a_stem_matching_two_files_splits_its_vote(self, tmp_path, extractor):
        for directory in ("a", "b"):
            (tmp_path / directory).mkdir()
            (tmp_path / directory / "util.py").write_text(
                "def helper():\n    return 1\n", encoding="utf-8"
            )

        scan = refs.scan_repo(tmp_path, extractor)
        index = refs.build_ref_index(scan)
        seeding = seed_weights(("util",), scan, index)

        assert seeding.unresolved == ()
        assert seeding.weights["a/util.py"] == pytest.approx(0.5)
        assert seeding.weights["b/util.py"] == pytest.approx(0.5)

    def test_a_qualified_method_name_narrows_to_its_owner(self, repo, extractor, counter):
        maps = MapService(extractor, counter)
        bare = maps.build(repo, MapRequest(focus=("cost_of",)))
        qualified = maps.build(repo, MapRequest(focus=("PriceBook.cost_of",)))
        assert bare.seeds == qualified.seeds == ("core.py",)

    def test_an_unresolved_seed_is_named_rather_than_dropped(self, repo, extractor, counter):
        maps = MapService(extractor, counter)
        result = maps.build(repo, MapRequest(focus=("lonely", "rotate_age", "  ")))
        assert result.seeds == ("orphan.py",)
        assert result.unresolved_seeds == ("rotate_age",)
        assert "rotate_age" in maps.render_text(result).splitlines()[0]
        assert result.as_dict()["seeds"] == ["orphan.py"]
        assert result.as_dict()["unresolved_seeds"] == ["rotate_age"]

    def test_the_budget_skips_files_the_focus_never_reaches(self, repo, extractor, counter):
        """A file no path connects to the seed is listed, not expanded.

        ``orphan.py`` has no edge to the other three, which form their own
        connected component. Before this, the ranking put all four in the
        answer and the packing search spent the budget expanding symbols from
        the three the caller had not asked about -- measured on this
        repository with `map --no-cache --focus
        contrib/hooks/agentless_gate_mark.py --max-files 10`, 13053
        characters against 3590.

        The files stay. Dropping them would be the bounded-view-mistaken-for-
        complete failure ``_group`` exists to prevent, and the tool
        description publishes a top-N of ten however large the repository is.
        What changes is where the budget goes.
        """
        result = MapService(extractor, counter).build(repo, MapRequest(focus=("lonely",)))
        by_path = {entry.path: entry for entry in result.files}

        assert result.seeds == ("orphan.py",)
        assert set(by_path) == {"orphan.py", "core.py", "billing.py", "ledger.py"}
        assert by_path["orphan.py"].shown > 0
        for path in ("core.py", "billing.py", "ledger.py"):
            # Listed, with a true count of what it holds, and no symbol of it
            # rendered.
            assert by_path[path].shown == 0, path
            assert by_path[path].omitted > 0, path

    def test_an_unreached_file_does_not_offer_the_budget_that_cannot_help(
        self, repo, extractor, counter
    ):
        """The per-file line has to agree with the banner above it.

        Excluding these symbols from `symbols_available` fixed the banner's
        "raise the budget for the rest" and left each unreached file's own
        row saying the same false thing in the ordinary omission wording.
        Both now say what is true of the file they describe.
        """
        maps = MapService(extractor, counter)
        text = maps.render_text(maps.build(repo, MapRequest(focus=("lonely",))))

        assert "the focus has no reference path to it" in text
        assert "more symbols in this file not listed" not in text

    def test_an_unfocused_map_keeps_the_ordinary_omission_wording(self, repo, extractor, counter):
        """A uniform walk reaches everything, so no file gets the other line."""
        maps = MapService(extractor, counter)
        text = maps.render_text(maps.build(repo, MapRequest(budget=200)))

        assert "the focus has no reference path to it" not in text

    def test_the_shown_of_total_count_describes_only_what_competed(self, repo, extractor, counter):
        """The banner offers "raise the budget for the rest", so the rest must exist.

        No budget renders a symbol in a file the focus never reached, so
        counting those in the total makes that advice false.
        """
        maps = MapService(extractor, counter)
        focused = maps.build(repo, MapRequest(focus=("lonely",)))
        unfocused = maps.build(repo, MapRequest())

        orphan_symbols = sum(
            entry.shown + entry.omitted for entry in focused.files if entry.path == "orphan.py"
        )
        assert focused.candidates == orphan_symbols
        # An unfocused walk reaches every file, so nothing is held back and
        # the number is the whole scored set -- which is what keeps the
        # goldens, all unfocused, unchanged.
        assert unfocused.candidates == sum(entry.shown + entry.omitted for entry in unfocused.files)

    def test_the_json_form_says_which_files_the_focus_reached(self, repo, extractor, counter):
        """Text and JSON are two doors onto one map, so both carry the fact.

        `omitted` alone cannot answer what a caller asks it -- would a bigger
        budget show the rest? The text render answers by branching on
        `reached` for its omission line; for one release the JSON emitted the
        same shape either way, so a consumer reading the map as data (the
        ranking harness does) had to match on a message to tell them apart.
        """
        result = MapService(extractor, counter).build(repo, MapRequest(focus=("lonely",)))
        by_path = {entry["path"]: entry for entry in result.as_dict()["files"]}

        assert by_path["orphan.py"]["reached"] is True
        for path in ("core.py", "billing.py", "ledger.py"):
            assert by_path[path]["reached"] is False, path
            assert by_path[path]["omitted"] > 0, path

    def test_a_file_granularity_map_marks_an_unreached_file_too(self, repo, extractor, counter):
        """The granularity where the wrong line was offered most, not least.

        Every file here has its whole symbol count omitted, so every file
        takes an omission line -- and this view has no budget at all to
        raise. Left to `MapFile.reached`'s default the answer told a caller
        to raise one, about a file no budget could reach.
        """
        maps = MapService(extractor, counter)
        result = maps.build(repo, MapRequest(focus=("lonely",), granularity=GRANULARITY_FILE))
        by_path = {entry.path: entry for entry in result.files}

        assert by_path["orphan.py"].reached is True
        assert "the focus has no reference path to it" in maps.render_text(result)
        for path in ("core.py", "billing.py", "ledger.py"):
            assert by_path[path].reached is False, path
        assert [entry["reached"] for entry in result.as_dict()["files"]].count(False) == 3

    def test_an_unfocused_file_granularity_map_reaches_everything(self, repo, extractor, counter):
        """The uniform walk reaches every file, so none takes the other line."""
        maps = MapService(extractor, counter)
        result = maps.build(repo, MapRequest(granularity=GRANULARITY_FILE))

        assert all(entry.reached for entry in result.files)
        assert "the focus has no reference path to it" not in maps.render_text(result)

    def test_a_fully_resolved_focus_adds_no_note(self, repo, extractor, counter):
        maps = MapService(extractor, counter)
        result = maps.build(repo, MapRequest(focus=("lonely",)))
        assert result.unresolved_seeds == ()
        assert not maps.render_text(result).startswith("// note:")

    def test_a_mistyped_path_does_not_resolve_through_its_extension(self, repo, extractor, counter):
        """`lib/nope.py` must not seed on a symbol that happens to be named `py`."""
        result = MapService(extractor, counter).build(repo, MapRequest(focus=("lib/nope.py",)))
        assert result.seeds == ()
        assert result.unresolved_seeds == ("lib/nope.py",)

    def test_one_focus_entry_casts_one_vote_however_many_files_it_matched(
        self, tmp_path, extractor
    ):
        """A common name matching three files must not outweigh a named file."""
        for name in ("a.py", "b.py", "c.py", "d.py"):
            (tmp_path / name).write_text("def handle():\n    return 1\n", encoding="utf-8")

        scan = refs.scan_repo(tmp_path, extractor)
        index = refs.build_ref_index(scan)
        seeding = seed_weights(("handle", "d.py"), scan, index)

        assert seeding.unresolved == ()
        assert seeding.weights["d.py"] == pytest.approx(1.25)
        assert seeding.weights["a.py"] == pytest.approx(0.25)
        assert sum(seeding.weights.values()) == pytest.approx(2.0)

    def test_max_files_caps_the_file_stage(self, repo, extractor, counter):
        result = MapService(extractor, counter).build(repo, MapRequest(max_files=2))
        assert len(result.files) <= 2

    def test_file_granularity_lists_paths_without_symbols(self, repo, extractor, counter):
        result = MapService(extractor, counter).build(
            repo, MapRequest(granularity=GRANULARITY_FILE)
        )
        assert all(entry.entries == () for entry in result.files)
        assert any(entry.omitted for entry in result.files)

    def test_the_auto_budget_stays_inside_the_documented_band(self, repo, extractor, counter):
        result = MapService(extractor, counter).build(repo, MapRequest())
        assert AUTO_BUDGET_MIN <= result.budget <= AUTO_BUDGET_MAX

    def test_a_file_reports_the_symbols_that_did_not_fit(self, tmp_path, extractor, counter):
        """Partial packing still reports what it left out, at a legal budget.

        This asked for ``budget=120``, which no door accepts any more:
        ``projectconfig`` declares 200..64000 and ``MapService`` now holds
        callers to it. The behaviour under test is unaffected -- a budget can
        still fit some symbols and not others -- so the repository is sized so
        that the smallest legal budget lands mid-way through it. Raising the
        floor did not make this path unreachable, and this test is the
        evidence.

        Resized from twelve files to six when the map dropped its line-number
        gutter for an ``@line`` suffix and gained a per-file id-pattern line.
        Twelve file headers plus their omission lines render to 182 of the 200
        tokens on their own, which left less than one symbol *and* its file's
        pattern line of room -- the budget stopped landing mid-way and started
        landing before the first symbol. Six files restore the boundary this
        pins: measured 5 of 72 symbols included at 188 tokens.
        """
        for index in range(6):
            body = "".join(
                f"def function_number_{index}_{inner}():\n    return {inner}\n\n"
                for inner in range(12)
            )
            (tmp_path / f"module_{index:02d}.py").write_text(body, encoding="utf-8")
        repo = resolve_repo(tmp_path, None)

        result = MapService(extractor, counter).build(repo, MapRequest(budget=MIN_BUDGET))

        assert 0 < result.included < result.candidates
        assert sum(entry.omitted for entry in result.files) > 0

    def test_two_builds_of_an_unchanged_repository_are_identical(self, repo, extractor, counter):
        maps = MapService(extractor, counter)
        assert maps.render_text(maps.build(repo, MapRequest())) == maps.render_text(
            maps.build(repo, MapRequest())
        )


class TestViewService:
    def test_skeleton_elides_bodies_and_keeps_signatures(self, repo, extractor):
        view = ViewService(extractor).skeleton(repo, ["core.py"])[0]
        assert "def quote(sku):" in view.text
        assert "return RATE" not in view.text

    def test_a_path_outside_the_repository_is_refused_per_file(self, repo, extractor):
        """A batch reports the refusal; it does not throw away the batch.

        `skeleton` used to raise, so one bad path discarded every other file
        the caller named and left them unable to tell which one did it. The
        rule `SymbolService._card` states over the same containment check --
        a batch reports per item, a single-target view raises -- now holds
        here too.
        """
        views = ViewService(extractor).skeleton(repo, ["core.py", "../escape.py"])

        assert [view.path for view in views] == ["core.py", "../escape.py"]
        assert "def quote(sku):" in views[0].text
        assert views[1].refused
        assert not views[0].refused

    def test_a_refusal_is_marked_apart_from_a_file_that_cannot_be_read(self, repo, extractor):
        """The two are different outcomes and the adapter maps them differently."""
        views = ViewService(extractor).skeleton(repo, ["../escape.py", "gone.py"])

        assert [view.refused for view in views] == [True, False]
        assert all(view.error for view in views)

    def test_an_unparseable_file_type_is_reported_per_file(self, repo, extractor):
        (repo.root / "notes.md").write_text("# hi\n", encoding="utf-8")
        view = ViewService(extractor).skeleton(repo, ["notes.md"])[0]
        assert view.text == ""
        assert "no grammar" in view.error

    def test_a_slice_carries_its_enclosing_signature(self, repo, extractor):
        view = ViewService(extractor).read_slice(repo, "ledger.py", intervals=[(6, 6)], context=0)
        assert "class Ledger:" in view.text
        assert "def post(self, item):" in view.text

    def test_a_slice_beyond_the_file_is_reported_not_rendered(self, repo, extractor):
        view = ViewService(extractor).read_slice(
            repo, "ledger.py", intervals=[(9000, 9050)], context=0
        )
        assert "unsatisfiable: line range 9000-9050 is beyond ledger.py (9 lines)" in view.text
        assert view.error == ""
        assert "class Ledger:" not in view.text
        assert "1|" not in view.text

    def test_a_slice_running_past_the_end_keeps_its_clamped_tail(self, repo, extractor):
        view = ViewService(extractor).read_slice(
            repo, "ledger.py", intervals=[(9, 9000)], context=0
        )
        assert "9|         return quote(item)" in view.text
        assert "unsatisfiable" not in view.text

    def test_a_bad_interval_does_not_hide_the_good_ones(self, repo, extractor):
        view = ViewService(extractor).read_slice(
            repo, "ledger.py", intervals=[(6, 6), (9000, 9050)], context=0
        )
        assert "6|         return normalise(quote(item))" in view.text
        assert "unsatisfiable: line range 9000-9050 is beyond ledger.py (9 lines)" in view.text

    def test_a_slice_with_no_ranges_still_returns_the_whole_file(self, repo, extractor):
        view = ViewService(extractor).read_slice(repo, "ledger.py")
        assert "1| from core import normalise, quote" in view.text
        assert "9|         return quote(item)" in view.text

    def test_resolve_locations_returns_ids_intervals_and_reasons(self, repo, extractor):
        view = ViewService(extractor).resolve_locations(
            repo, "core.py", ["class: PriceBook", "function: nope"]
        )
        assert view.resolution.stable_ids == ("py:core.py::PriceBook",)
        assert view.resolution.intervals
        assert view.resolution.unrecognized[0].loc == "function: nope"


class TestSliceRangesAreValidatedHere:
    """A range the file cannot satisfy is named, never answered with the file.

    The service owns the check rather than one adapter: the CLI validated
    these and the MCP surface did not, so every shape below used to come back
    as the whole file -- the token blow-up the funnel exists to prevent, and a
    false belief that the lines returned are the ones asked for.
    """

    def test_a_transposed_range_is_refused_rather_than_rendered_whole(self, repo, extractor):
        view = ViewService(extractor).read_slice(repo, "ledger.py", intervals=[(8, 3)], context=0)
        assert "line range 8-3 is not a range" in view.text
        assert "1| from core import normalise, quote" not in view.text

    def test_a_range_starting_below_one_is_refused(self, repo, extractor):
        view = ViewService(extractor).read_slice(repo, "ledger.py", intervals=[(-5, -1)], context=0)
        assert "line range -5--1 is not a range" in view.text
        assert "1| from core import normalise, quote" not in view.text

    def test_a_negative_context_is_refused_by_name(self, repo, extractor):
        with pytest.raises(AgentlessError, match="context takes a value from 0 through 200"):
            ViewService(extractor).read_slice(repo, "ledger.py", intervals=[(5, 5)], context=-50)

    def test_resolve_locations_refuses_a_negative_context_too(self, repo, extractor):
        with pytest.raises(AgentlessError, match="context takes a value from 0 through 200"):
            ViewService(extractor).resolve_locations(repo, "core.py", ["line: 10"], context=-50)

    def test_one_bad_range_beside_a_good_one_still_renders_the_good_one(self, repo, extractor):
        view = ViewService(extractor).read_slice(
            repo, "ledger.py", intervals=[(6, 6), (8, 3)], context=0
        )
        assert "6|         return normalise(quote(item))" in view.text
        assert "line range 8-3 is not a range" in view.text


class TestTheReportedLineCountIsTheFilesOwn:
    """A trailing newline is a line terminator, not a line.

    Every source file ends in one, so counting it made the "true line count"
    this package states to agents one too high everywhere: a whole-file slice
    ended in a blank numbered line that is not in the file, and the line after
    the last one resolved instead of being refused.
    """

    def test_a_whole_file_slice_has_no_phantom_last_line(self, repo, extractor):
        view = ViewService(extractor).read_slice(repo, "ledger.py")
        assert view.text.rstrip("\n").endswith("9|         return quote(item)")
        assert "10|" not in view.text

    def test_the_out_of_range_message_states_the_real_count(self, repo, extractor):
        view = ViewService(extractor).read_slice(repo, "ledger.py", intervals=[(10, 10)], context=0)
        assert "is beyond ledger.py (9 lines)" in view.text

    def test_the_line_after_the_last_one_is_refused_not_resolved(self, repo, extractor):
        view = ViewService(extractor).resolve_locations(repo, "ledger.py", ["line: 10"])
        assert view.resolution.spans == ()
        assert "line 10 is outside ledger.py (1-9)" in view.resolution.unrecognized[0].reason


class TestSymbolService:
    def test_find_symbol_ranks_an_exact_name_first(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_symbol(repo, "quote")
        assert result.cards[0].stable_id == "py:core.py::quote"

    def test_find_symbol_filters_by_kind(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_symbol(repo, "o", kind="method")
        assert {card.kind for card in result.cards} == {"method"}

    def test_find_symbol_reports_the_total_behind_the_limit(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_symbol(repo, "", limit=2)
        assert len(result.cards) == 2
        assert result.total > 2

    def test_a_find_miss_names_the_next_step(self, repo, extractor):
        """A miss with no remediation is the one dead end in the surface.

        Every refusal in the message catalog names what to do next; a bare
        "no matching symbols" did not. The usual cause is that the name is a
        parameter, a local variable, an attribute or a string literal rather
        than a declared symbol, and the remedy for those is a text search.
        """
        result = SymbolService(extractor, Chars4Counter()).find_symbol(repo, "frobnicate")
        text = render_find(result)
        assert "no matching symbols" in text
        assert "frobnicate" in text
        assert "grep" in text

    def test_a_kind_blocked_miss_names_the_kinds_that_matched(self, repo, extractor):
        """The service knows the filter caused the miss, so the answer says so.

        "quote" matches a function; asked for a class, the honest answer is
        not "no matching symbols" but "not under that kind", naming the kinds
        the name did match so the next call needs no guessing.
        """
        result = SymbolService(extractor, Chars4Counter()).find_symbol(repo, "quote", kind="class")
        assert result.total == 0
        assert result.other_kinds == ("function",)
        text = render_find(result)
        assert "class" in text
        assert "function" in text

    def test_a_miss_that_no_kind_would_save_takes_the_plain_wording(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_symbol(
            repo, "frobnicate", kind="class"
        )
        assert result.other_kinds == ()
        assert "grep" in render_find(result)

    def test_expand_reports_an_id_whose_symbol_is_gone(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).expand_symbols(
            repo, ["py:core.py::vanished"]
        )
        assert result.cards == ()
        assert "no longer defines" in result.unresolved[0][1]

    def test_expand_refuses_an_id_pointing_out_of_the_repository(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).expand_symbols(
            repo, ["py:../../etc/passwd::x"]
        )
        assert result.cards == ()
        assert "refused" in result.unresolved[0][1]

    def test_refs_group_by_file_and_name_the_calling_symbol(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(repo, "quote")
        callers = {(group.path, site.enclosing) for group in result.groups for site in group.sites}
        assert ("billing.py", "run_billing") in callers
        assert ("ledger.py", "Ledger.post") in callers
        assert ("core.py", "PriceBook.cost_of") in callers

    def test_a_group_mixing_module_level_and_symbol_sites_still_prints_one_pattern(
        self, tmp_path, extractor
    ):
        """The prefix is read off whichever site has a symbol, not the first one.

        `caller.py` references the target twice: once from its import line,
        which sits inside no symbol, and once from a function. The
        module-level row is first, and it carries no symbol to take a
        language prefix from -- so a pattern line built from site one alone
        would be missing, and the function row below it would have no id an
        agent could rebuild.
        """
        (tmp_path / "core.py").write_text("def quote(sku):\n    return sku\n", encoding="utf-8")
        (tmp_path / "caller.py").write_text(
            "from core import quote\n\n\ndef ask():\n    return quote('a')\n", encoding="utf-8"
        )
        ctx = resolve_repo(tmp_path, None)

        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(ctx, "quote")
        group = next(entry for entry in result.groups if entry.path == "caller.py")
        text = render.render_ref_groups(list(result.groups), "quote")

        assert group.id_prefix == "py"
        assert [site.enclosing for site in group.sites] == [render.MODULE_LEVEL, "ask"]
        assert "stable ids: py:caller.py::<QualifiedName>" in text
        # The row with no symbol has no id to shorten, and says so rather
        # than printing a name the pattern cannot complete.
        assert f"  {render.MODULE_LEVEL} @1" in text
        assert "  [ask] @5" in text

    def test_a_name_spelled_twice_in_one_file_keeps_its_ordinal_on_the_row(
        self, tmp_path, extractor
    ):
        """The one case where the row's name and the enclosing name differ.

        A second `quote` takes a `#2` ordinal in its id, and that is the
        spelling the row has to carry: the pattern line plus a bare `quote`
        would rebuild the id of the *first* one. Driven through the service
        rather than the renderer, because the ordinal is minted upstream.
        """
        (tmp_path / "core.py").write_text(
            "def helper():\n    return 1\n\n\n"
            "def quote(sku):\n    return helper()\n\n\n"
            "def quote(sku, tier):\n    return helper()\n",
            encoding="utf-8",
        )
        ctx = resolve_repo(tmp_path, None)

        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(ctx, "helper")
        names = {site.name for group in result.groups for site in group.sites}
        text = render.render_ref_groups(list(result.groups), "helper")

        assert {"quote", "quote#2"} <= names
        assert "  [quote#2] @" in text

    def test_refs_accept_a_stable_id_as_the_target(self, repo, extractor):
        by_name = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(repo, "quote")
        by_id = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "py:core.py::quote"
        )
        assert by_id.total == by_name.total

    def test_refs_of_an_unknown_name_are_empty_not_an_error(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "nosuchsymbol"
        )
        assert list(result.groups) == []
        assert result.total == 0

    def test_shared_callers_surfaces_a_symbol_the_same_callers_use(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            repo, "quote", shared_callers=True
        )
        assert [row.stable_id for row in result.shared] == ["py:core.py::normalise"]
        row = result.shared[0]
        assert (row.path, row.line) == ("core.py", 10)
        assert (row.overlap, row.shared_files) == (2, 2)
        assert [(caller.qualname, caller.path, caller.line) for caller in row.callers] == [
            ("run_billing", "billing.py", 4),
            ("Ledger.post", "ledger.py", 5),
        ]


# One helper called from six files, so a limit that bites cuts whole files
# out of the answer -- the shape a blast-radius question is actually asked in.
WIDE_CALLER = """\
from core import widget


def use_{index}():
    first = widget()
    return first + widget()
"""

WIDE_CALLER_FILES = 6
WIDE_SITES_PER_FILE = 3


@pytest.fixture
def wide_fan_in(tmp_path):
    """A repository whose one helper is referenced from every other file."""
    (tmp_path / "core.py").write_text("def widget():\n    return 1\n", encoding="utf-8")
    for index in range(WIDE_CALLER_FILES):
        (tmp_path / f"caller_{index}.py").write_text(
            WIDE_CALLER.format(index=index), encoding="utf-8"
        )
    return resolve_repo(tmp_path, None)


class TestABoundedListingSaysWhatItLeftOut:
    """The rule this package is built on, applied to the two views that broke it.

    A service that slices returns the pre-slice total, and the renderer that
    receives the sliced sequence receives that total with it. Fan-in used to
    recompute its header from the rows that survived, so fifty-two reference
    sites at a limit of ten rendered as "10 references to widget" with no
    marker -- an agent reads that as the complete caller set.
    """

    def test_a_truncated_fan_in_names_the_sites_and_files_it_dropped(self, wide_fan_in, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            wide_fan_in, "widget", limit=4
        )
        sites = WIDE_CALLER_FILES * WIDE_SITES_PER_FILE
        rendered = render.render_ref_groups(result.groups, "widget")

        assert result.total == sites
        assert rendered.startswith(f"{sites} references to widget")
        assert "caller_0.py:4" not in rendered
        assert f"... {sites - 4} more references not listed" in rendered
        # Four sites are spent one per file, so four of the six files are
        # represented and two are cut out whole.
        assert "including every reference in 2 more files" in rendered

    def test_the_json_form_carries_the_same_counts_as_the_text(self, wide_fan_in, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            wide_fan_in, "widget", limit=4
        )
        document = result.as_dict()

        assert document["total"] == WIDE_CALLER_FILES * WIDE_SITES_PER_FILE
        assert document["omitted"] == document["total"] - 4
        assert document["files"] == WIDE_CALLER_FILES
        assert document["files_omitted"] == WIDE_CALLER_FILES - len(document["groups"])

    def test_an_untruncated_fan_in_says_nothing_about_omissions(self, wide_fan_in, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
            wide_fan_in, "widget"
        )
        rendered = render.render_ref_groups(result.groups, "widget")

        assert result.groups.omitted == 0
        assert "not listed" not in rendered

    def test_a_truncated_lookup_names_the_matches_it_dropped(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_symbol(repo, "", limit=2)
        rendered = render.render_symbol_cards(result.cards)

        assert result.cards.omitted == result.total - 2
        assert f"... {result.total - 2} more matches not listed (limit 2)" in rendered

    def test_an_untruncated_lookup_says_nothing_about_omissions(self, repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_symbol(repo, "quote")
        assert "not listed" not in render.render_symbol_cards(result.cards)


class TestALimitThatBoundsNothingIsRefused:
    """`limit=0` used to answer "no references" for a symbol with fifty-two."""

    def test_find_symbol_refuses_a_zero_limit(self, repo, extractor):
        with pytest.raises(AgentlessError, match="limit takes a value from 1 through 500"):
            SymbolService(extractor, Chars4Counter()).find_symbol(repo, "quote", limit=0)

    def test_fan_in_refuses_a_zero_limit(self, repo, extractor):
        with pytest.raises(AgentlessError, match="limit takes a value from 1 through 500"):
            SymbolService(extractor, Chars4Counter()).find_referencing_symbols(
                repo, "quote", limit=0
            )

    def test_expand_refuses_a_negative_limit(self, repo, extractor):
        with pytest.raises(AgentlessError, match="limit takes a value from 1 through 500"):
            SymbolService(extractor, Chars4Counter()).expand_symbols(
                repo, ["py:core.py::quote"], limit=-1
            )


class TestOneFileFailsAlone:
    def test_an_unwarmed_grammar_degrades_one_id_not_the_batch(self, repo, extractor, monkeypatch):
        """`core.refs._parse_one` sets the convention; expansion broke it."""
        (repo.root / "helper.go").write_text(
            "package main\n\nfunc Helper() int {\n\treturn 1\n}\n", encoding="utf-8"
        )
        warmed = grammars.get_parser

        def unwarmed(name):
            if name == "go":
                message = "language 'go' not warmed: run agentless-mcp warmup"
                raise LanguageUnavailable(message)
            return warmed(name)

        monkeypatch.setattr(grammars, "get_parser", unwarmed)

        result = SymbolService(extractor, Chars4Counter()).expand_symbols(
            repo, ["py:core.py::quote", "go:helper.go::Helper"]
        )

        assert [card.stable_id for card in result.cards] == ["py:core.py::quote"]
        assert result.unresolved[0][0] == "go:helper.go::Helper"
        assert "warmup" in result.unresolved[0][1]


# A real multi-line declaration of the shape that broke the map on a live
# repository: the dedicated Python handler took `signature` verbatim from the
# source, so one symbol rendered across six physical lines and spent the
# budget doing it.
WIDE = '''\
"""Storage."""

from typing import Any, Protocol


class VectorStoreProtocol(Protocol):
    def search(
        self,
        query: str,
        limit: int = 10,
        filters: dict[str, Any] | None = None,
        include_metadata: bool = True,
    ) -> list[dict[str, Any]]:
        ...


def build_a_storage_client_with_a_very_long_name(
    endpoint: str,
    credentials: dict[str, str],
    retries: int = 3,
) -> VectorStoreProtocol:
    raise NotImplementedError
'''


@pytest.fixture
def wide_repo(tmp_path):
    """A repository whose declarations span several source lines."""
    (tmp_path / "storage.py").write_text(WIDE, encoding="utf-8")
    return resolve_repo(tmp_path, None)


class TestSignaturesStayOneLine:
    """Regression: every renderer assumes one symbol per line.

    Asserted on the rendered output, not just on the field, because the field
    being right is only useful if what reaches the reader is too.
    """

    def test_the_map_renders_one_line_per_symbol(self, wide_repo, extractor, counter):
        maps = MapService(extractor, counter)
        result = maps.build(wide_repo, MapRequest())

        entries = [entry for map_file in result.files for entry in map_file.entries]
        assert entries
        for entry in entries:
            assert "\n" not in entry.signature
            assert len(entry.signature) <= SIGNATURE_MAX_CHARS

        body = maps.render_text(result)
        printed = [line for line in body.splitlines() if "  [" in line]
        assert len(printed) == len(entries)

    def test_the_multi_line_def_is_collapsed_not_dropped(self, wide_repo, extractor, counter):
        result = MapService(extractor, counter).build(wide_repo, MapRequest())
        search = next(
            entry
            for map_file in result.files
            for entry in map_file.entries
            if entry.stable_id.endswith("::VectorStoreProtocol.search")
        )
        assert search.signature.startswith("def search( self, query: str,")
        assert len(search.signature) <= SIGNATURE_MAX_CHARS

    def test_an_over_long_signature_is_marked_as_cut(self, wide_repo, extractor):
        card = (
            SymbolService(extractor, Chars4Counter())
            .find_symbol(wide_repo, "build_a_storage_client")
            .cards[0]
        )
        assert len(card.signature) == SIGNATURE_MAX_CHARS
        assert card.signature.endswith("...")
        assert "\n" not in card.signature

    def test_find_symbol_cards_stay_one_line(self, wide_repo, extractor):
        result = SymbolService(extractor, Chars4Counter()).find_symbol(wide_repo, "search")
        card = result.cards[0]
        assert "\n" not in card.signature
        assert len(card.signature) <= SIGNATURE_MAX_CHARS
        rendered = render.render_symbol_cards([card])
        assert "[py:storage.py::VectorStoreProtocol.search] @" in rendered
        assert "storage.py:7" not in rendered
        assert len(rendered.rstrip("\n").splitlines()) == 3

    def test_expand_still_shows_the_real_multi_line_source(self, wide_repo, extractor):
        """Capping the signature must not cap the body: expand is the escalation."""
        card = (
            SymbolService(extractor, Chars4Counter())
            .expand_symbols(wide_repo, ["py:storage.py::VectorStoreProtocol.search"])
            .cards[0]
        )
        assert "7|     def search(" in card.body
        assert "9|         query: str," in card.body

    def test_the_skeleton_still_renders_verbatim_source(self, wide_repo, extractor):
        """The skeletonizer reads spans, not the signature field, and must stay that way."""
        text = ViewService(extractor).skeleton(wide_repo, ["storage.py"])[0].text
        assert "    def search(\n" in text
        assert "        query: str,\n" in text
