"""What an expansion does with an id it cannot honour, and what a fan-in says.

The cards themselves are covered in ``test_services``. What is pinned here is
the accounting around them: an id whose language prefix names another file's
language, an id sent twice, and the ids one call has no room for -- each of
which used to be answered by quietly doing something else.
"""

import json

import pytest

from agentless_mcp.application import envelope
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.symbol_service import (
    EXPAND_MAX_SEATS,
    SymbolService,
)
from agentless_mcp.util.errors import AgentlessError

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
