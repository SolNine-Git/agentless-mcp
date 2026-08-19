"""The `--shared-callers` adjacency pass: ranking, damping and file:line.

The DRY question this answers -- "do we already have a utility for this?" --
only pays off if the answer is ordered. The fixture below is built so that the
wrong ordering is visible. Three symbols compete for the same query:

* ``format_currency`` -- the genuine answer. Two of the target's callers use
  it, and it appears in three files in total.
* ``log`` -- an incidentally shared common name. *Three* of the callers use
  it, so raw overlap ranks it first, but it appears in every file in the
  repository, and the log damping on its spread is what pushes it back down.
* ``fx`` -- a two-character name, damped by the noise-name multiplier for the
  same reason short identifiers are damped in the reference graph.

Ranking by raw overlap puts the common name first, which is the failure this
suite pins. Both corrections are the treatment
:mod:`agentless_mcp.core.graph` already applies to its edge weights, so the
two views cannot drift apart.
"""

import pytest

from agentless_mcp.application import render
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.core import projectconfig
from agentless_mcp.util.tokens import Chars4Counter

# How many files mention `log` and nothing else. Enough that its spread across
# the repository is unmistakable rather than marginal.
NOISE_FILES = 20

UTIL = '''\
"""Utilities."""


def log(message):
    return message


def fx(amount):
    return amount


def format_currency(amount):
    return f"{amount:.2f}"


def quote(sku):
    return 1
'''

BILLING = """\
from util import format_currency, fx, log, quote


def run_billing(items):
    log("billing")
    return [format_currency(fx(quote(item))) for item in items]
"""

LEDGER = """\
from util import format_currency, fx, log, quote


def post(item):
    log("posting")
    return format_currency(fx(quote(item)))
"""

# A third caller of the target that uses the common names but not the helper,
# which is what gives `log` and `fx` the higher raw overlap.
TALLY = """\
from util import fx, log, quote


def tally(items):
    log("tally")
    return sum(fx(quote(item)) for item in items)
"""

NOISE = """\
from util import log


def handle_{index}(item):
    return log(item)
"""


@pytest.fixture
def repo(tmp_path):
    """A repository with one genuinely shared helper and two noisy names."""
    (tmp_path / "util.py").write_text(UTIL, encoding="utf-8")
    (tmp_path / "billing.py").write_text(BILLING, encoding="utf-8")
    (tmp_path / "ledger.py").write_text(LEDGER, encoding="utf-8")
    (tmp_path / "tally.py").write_text(TALLY, encoding="utf-8")
    for index in range(NOISE_FILES):
        (tmp_path / f"noise_{index}.py").write_text(NOISE.format(index=index), encoding="utf-8")
    return resolve_repo(tmp_path, None)


def rows(repo, extractor, target="quote"):
    """Return the adjacency rows for one target, ranked."""
    return (
        SymbolService(extractor, Chars4Counter())
        .find_referencing_symbols(repo, target, shared_callers=True)
        .shared
    )


class TestRanking:
    def test_a_genuinely_shared_utility_outranks_a_common_name(self, repo, extractor):
        ranked = [row.stable_id for row in rows(repo, extractor)]
        assert ranked.index("py:util.py::format_currency") < ranked.index("py:util.py::log")

    def test_the_common_name_still_shares_strictly_more_raw_callers(self, repo, extractor):
        # The damping is doing the work, not a filter: ranking on `overlap`
        # alone would put `log` first, and it does share more callers.
        by_id = {row.stable_id: row for row in rows(repo, extractor)}
        assert by_id["py:util.py::log"].overlap > by_id["py:util.py::format_currency"].overlap
        assert by_id["py:util.py::log"].score < by_id["py:util.py::format_currency"].score

    def test_a_two_character_name_is_damped_as_noise(self, repo, extractor):
        by_id = {row.stable_id: row for row in rows(repo, extractor)}
        assert by_id["py:util.py::fx"].overlap > by_id["py:util.py::format_currency"].overlap
        assert by_id["py:util.py::fx"].score < by_id["py:util.py::format_currency"].score

    def test_rows_are_ordered_by_descending_score(self, repo, extractor):
        scores = [row.score for row in rows(repo, extractor)]
        assert scores == sorted(scores, reverse=True)

    def test_shared_files_counts_distinct_caller_files(self, repo, extractor):
        by_id = {row.stable_id: row for row in rows(repo, extractor)}
        row = by_id["py:util.py::format_currency"]
        assert row.shared_files == len({caller.path for caller in row.callers})
        assert row.shared_files == 2

    def test_a_stoplisted_name_is_damped_like_a_short_one(self, repo, extractor, tmp_path):
        before = {row.stable_id: row.score for row in rows(repo, extractor)}

        (tmp_path / projectconfig.CONFIG_FILENAME).write_text(
            '{"stoplist": ["format_currency"]}', encoding="utf-8"
        )
        after = {row.stable_id: row.score for row in rows(resolve_repo(tmp_path, None), extractor)}

        assert after["py:util.py::format_currency"] < before["py:util.py::format_currency"]
        assert after["py:util.py::log"] == pytest.approx(before["py:util.py::log"])


class TestRowShape:
    def test_every_row_carries_the_definition_file_and_line(self, repo, extractor):
        for row in rows(repo, extractor):
            assert row.path.endswith(".py")
            assert row.line >= 1

    def test_every_caller_carries_its_own_file_and_line(self, repo, extractor):
        for row in rows(repo, extractor):
            for caller in row.callers:
                assert caller.path.endswith(".py")
                assert caller.line >= 1
                assert caller.qualname

    def test_the_json_form_carries_the_ranking_inputs(self, repo, extractor):
        record = rows(repo, extractor)[0].as_dict()
        assert set(record) == {
            "stable_id",
            "path",
            "line",
            "overlap",
            "shared_files",
            "score",
            "defined_in_tests",
            "shared_callers",
        }
        assert record["shared_callers"][0].keys() == {"qualname", "path", "line"}

    def test_the_target_itself_is_never_an_adjacency_row(self, repo, extractor):
        assert "py:util.py::quote" not in {row.stable_id for row in rows(repo, extractor)}

    def test_a_symbol_shared_by_one_caller_only_is_not_reported(self, repo, extractor):
        # A single caller in common is a coincidence, not adjacency: the noise
        # modules call `log` and nothing else, and none of them makes it in.
        for row in rows(repo, extractor):
            assert row.overlap > 1


class TestLimit:
    def test_the_limit_bounds_the_candidates_and_counts_the_rest(self, repo, extractor):
        listing = (
            SymbolService(extractor, Chars4Counter())
            .find_referencing_symbols(repo, "quote", limit=2, shared_callers=True)
            .shared
        )
        assert listing.limit == 2
        assert len(listing) == 2
        assert listing.total == 3
        assert listing.omitted == 1

    def test_the_kept_rows_are_the_strongest_ones(self, repo, extractor):
        everything = [row.stable_id for row in rows(repo, extractor)]
        capped = (
            SymbolService(extractor, Chars4Counter())
            .find_referencing_symbols(repo, "quote", limit=2, shared_callers=True)
            .shared
        )
        assert [row.stable_id for row in capped] == everything[:2]

    def test_the_default_listing_reports_no_omission(self, repo, extractor):
        listing = (
            SymbolService(extractor, Chars4Counter())
            .find_referencing_symbols(repo, "quote", shared_callers=True)
            .shared
        )
        assert listing.omitted == 0
        assert listing.total == len(listing)


# ---------------------------------------------------------------------------
# Caller fan-out damping: the promiscuous-caller correction
# ---------------------------------------------------------------------------

SHOP = '''\
"""Shop helpers."""


def quote2(item):
    return 1


def helper_low(item):
    return item


def helper_high(item):
    return item
'''

# A caller that references exactly the target and one helper.
FOCUSED = """\
from shop import helper_low, quote2


def focused_{index}(item):
    return helper_low(quote2(item))
"""


def busy_module(index):
    """A caller with enormous fan-out, like a characterization-test builder."""
    extras = " + ".join(f"extra_{n}(item)" for n in range(20))
    return (
        "from shop import helper_high, quote2\n"
        "\n"
        "\n"
        f"def busy_{index}(item):\n"
        f"    return quote2(item) + helper_high(item) + {extras}\n"
    )


@pytest.fixture
def fanout_repo(tmp_path):
    """Two helpers that tie on every input except their callers' fan-out."""
    (tmp_path / "shop.py").write_text(SHOP, encoding="utf-8")
    for index in range(2):
        focused = FOCUSED.format(index=index)
        (tmp_path / f"focused_{index}.py").write_text(focused, encoding="utf-8")
        (tmp_path / f"busy_{index}.py").write_text(busy_module(index), encoding="utf-8")
    return resolve_repo(tmp_path, None)


class TestCallerFanOut:
    def test_a_promiscuous_caller_contributes_less_than_a_focused_one(self, fanout_repo, extractor):
        # The two helpers tie on every prior ranking input -- two shared
        # callers in two files each, same name length, same repository
        # spread -- so only the fan-out of the callers themselves can
        # separate the scores, and the focused callers must win.
        by_id = {row.stable_id: row for row in rows(fanout_repo, extractor, target="quote2")}
        low = by_id["py:shop.py::helper_low"]
        high = by_id["py:shop.py::helper_high"]
        assert (low.overlap, low.shared_files) == (high.overlap, high.shared_files)
        assert low.score > high.score


# ---------------------------------------------------------------------------
# Production candidates rank ahead of test-defined ones
# ---------------------------------------------------------------------------

PRICING = '''\
"""Pricing helpers."""


def quote3(item):
    return 1


def prod_helper(item):
    return item
'''

TEST_HELPERS = '''\
"""Builders shared by the test suite."""


def fixture_helper(item):
    return item
'''

# Two callers use both helpers; a third uses only the test-tree one, which is
# what hands `fixture_helper` the strictly better raw score.
CALLER_BOTH = """\
from pricing import prod_helper, quote3
from tests.helpers import fixture_helper


def caller_{index}(item):
    return prod_helper(quote3(item)) + fixture_helper(item)
"""

CALLER_TEST_ONLY = """\
from pricing import quote3
from tests.helpers import fixture_helper


def caller_2(item):
    return quote3(item) + fixture_helper(item)
"""


@pytest.fixture
def tests_repo(tmp_path):
    """A repository where a test-tree helper out-scores the production one."""
    (tmp_path / "pricing.py").write_text(PRICING, encoding="utf-8")
    (tmp_path / "tests").mkdir()
    (tmp_path / "tests" / "helpers.py").write_text(TEST_HELPERS, encoding="utf-8")
    for index in range(2):
        caller = CALLER_BOTH.format(index=index)
        (tmp_path / f"caller_{index}.py").write_text(caller, encoding="utf-8")
    (tmp_path / "caller_2.py").write_text(CALLER_TEST_ONLY, encoding="utf-8")
    return resolve_repo(tmp_path, None)


class TestProductionFirst:
    def test_a_test_defined_candidate_never_outranks_production(self, tests_repo, extractor):
        flags = [row.in_tests for row in rows(tests_repo, extractor, target="quote3")]
        # The test row is listed -- hiding it would misreport the repository --
        # but every production row comes first.
        assert True in flags
        assert flags == sorted(flags)

    def test_the_ordering_does_the_work_not_the_score(self, tests_repo, extractor):
        ranked = rows(tests_repo, extractor, target="quote3")
        by_id = {row.stable_id: row for row in ranked}
        helper = by_id["py:tests/helpers.py::fixture_helper"]
        prod = by_id["py:pricing.py::prod_helper"]
        assert helper.in_tests
        assert not prod.in_tests
        # Three shared callers to two: the test helper wins on score, and the
        # production row still ranks above it.
        assert helper.score > prod.score
        ordered = [row.stable_id for row in ranked]
        assert ordered.index(prod.stable_id) < ordered.index(helper.stable_id)


class TestRendering:
    @staticmethod
    def adjacency_row(caller_count, *, in_tests=False, stable_id="py:util.py::helper"):
        callers = tuple(
            render.CallerRef(qualname=f"caller_{n}", path=f"mod_{n}.py", line=n + 1)
            for n in range(caller_count)
        )
        return render.SharedCaller(
            stable_id=stable_id,
            path=stable_id.split("::")[0].removeprefix("py:"),
            line=3,
            overlap=caller_count,
            shared_files=caller_count,
            score=1.0,
            callers=callers,
            in_tests=in_tests,
        )

    def test_the_render_caps_each_caller_list_at_a_handful(self):
        listing = render.SharedCallerListing(rows=(self.adjacency_row(8),), total=1, limit=10)
        text = render.render_shared_callers(listing, "quote")
        assert "caller_4" in text
        assert "caller_5" not in text
        assert "... 3 more callers not listed" in text

    def test_the_render_counts_candidates_the_limit_left_out(self):
        listing = render.SharedCallerListing(rows=(self.adjacency_row(2),), total=5, limit=1)
        text = render.render_shared_callers(listing, "quote")
        assert "... 4 more candidates not listed" in text

    def test_test_defined_rows_sit_under_their_own_heading(self):
        prod = self.adjacency_row(2, stable_id="py:util.py::helper")
        tests = self.adjacency_row(2, in_tests=True, stable_id="py:tests/helpers.py::builder")
        listing = render.SharedCallerListing(rows=(prod, tests), total=2, limit=10)
        text = render.render_shared_callers(listing, "quote")
        heading = text.index("defined in tests")
        assert text.index("py:util.py::helper") < heading
        assert heading < text.index("py:tests/helpers.py::builder")
