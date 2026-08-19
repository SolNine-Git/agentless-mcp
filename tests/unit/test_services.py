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
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import refs
from agentless_mcp.core.symbols import SIGNATURE_MAX_CHARS
from agentless_mcp.util.errors import SecurityRefusal
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

    def test_a_fully_resolved_focus_adds_no_note(self, repo, extractor, counter):
        maps = MapService(extractor, counter)
        result = maps.build(repo, MapRequest(focus=("lonely",)))
        assert result.unresolved_seeds == ()
        assert not maps.render_text(result).startswith("# note:")

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

    def test_a_file_reports_the_symbols_that_did_not_fit(self, repo, extractor, counter):
        result = MapService(extractor, counter).build(repo, MapRequest(budget=120))
        assert result.included < result.candidates
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

    def test_a_path_outside_the_repository_is_refused(self, repo, extractor):
        with pytest.raises(SecurityRefusal):
            ViewService(extractor).skeleton(repo, ["../escape.py"])

    def test_an_unparseable_file_type_is_reported_per_file(self, repo, extractor):
        (repo.root / "notes.md").write_text("# hi\n", encoding="utf-8")
        view = ViewService(extractor).skeleton(repo, ["notes.md"])[0]
        assert view.text == ""
        assert "no grammar" in view.error

    def test_a_slice_carries_its_enclosing_signature(self, repo, extractor):
        view = ViewService(extractor).read_slice(repo, "ledger.py", intervals=[(6, 6)], context=0)
        assert "class Ledger:" in view.text
        assert "def post(self, item):" in view.text

    def test_resolve_locations_returns_ids_intervals_and_reasons(self, repo, extractor):
        view = ViewService(extractor).resolve_locations(
            repo, "core.py", ["class: PriceBook", "function: nope"]
        )
        assert view.resolution.stable_ids == ("py:core.py::PriceBook",)
        assert view.resolution.intervals
        assert view.resolution.unrecognized[0].loc == "function: nope"


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
        assert result.groups == ()
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
        assert len(render.render_symbol_cards([card]).rstrip("\n").splitlines()) == 3

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
