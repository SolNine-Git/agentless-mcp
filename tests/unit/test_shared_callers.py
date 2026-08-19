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

from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.core import projectconfig

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
        SymbolService(extractor).find_referencing_symbols(repo, target, shared_callers=True).shared
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
