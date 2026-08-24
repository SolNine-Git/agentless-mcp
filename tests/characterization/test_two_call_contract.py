"""The two-call contract: every id a map prints must expand.

This is the single property the recommended recipe rests on. If `map` prints
an id that `expand_symbols` cannot resolve, the escalation step fails at
exactly the moment an agent has committed to it, and the failure looks like
"the symbol does not exist" rather than "the id format drifted". Asserted per
fixture repository and across every language, because the id is built from
the extractor's own language and path fields and each handler fills those in
separately.
"""

import json
from pathlib import Path

import pytest

from agentless_mcp.application import envelope
from agentless_mcp.application.map_service import MapRequest, MapService
from agentless_mcp.application.repo_context import RepoContext, resolve_repo
from agentless_mcp.application.symbol_service import (
    EXPAND_BUDGET_TOKENS,
    EXPAND_MAX_SEATS,
    SymbolService,
    render_expansion,
)
from agentless_mcp.core.symbols import parse_stable_id
from agentless_mcp.util.tokens import Chars4Counter

FIXTURES = Path(__file__).parent / "fixtures"
REPOS = ("repo_py", "repo_ts", "repo_go")


def context_for(repo: str) -> RepoContext:
    """A pinned context for one fixture repository."""
    return RepoContext(
        root=(FIXTURES / repo).resolve(),
        head_sha="0000000f",
        tree_oid="1111111f",
        dirty_count=0,
        note="",
    )


@pytest.mark.parametrize("repo", REPOS)
class TestTwoCallContract:
    def test_every_id_the_map_prints_expands(self, repo, extractor, counter):
        ctx = context_for(repo)
        result = MapService(extractor, counter).build(ctx, MapRequest(budget=8_000))
        ids = [entry.stable_id for map_file in result.files for entry in map_file.entries]
        assert ids, "the map printed no symbols to expand"

        symbols = SymbolService(extractor, Chars4Counter())
        for batch in _batches(ids, 10):
            expanded = symbols.expand_symbols(ctx, batch)
            assert expanded.unresolved == (), f"{repo}: {expanded.unresolved}"
            assert len(expanded.cards) == len(batch)

    def test_every_expanded_body_carries_its_own_first_line(self, repo, extractor, counter):
        ctx = context_for(repo)
        result = MapService(extractor, counter).build(ctx, MapRequest(budget=8_000))
        first = next(entry for map_file in result.files for entry in map_file.entries)

        card = (
            SymbolService(extractor, Chars4Counter())
            .expand_symbols(ctx, [first.stable_id])
            .cards[0]
        )
        assert card.body.startswith(f"{first.line}| ")
        assert card.stable_id == first.stable_id

    def test_every_id_the_symbol_search_prints_expands(self, repo, extractor):
        ctx = context_for(repo)
        symbols = SymbolService(extractor, Chars4Counter())
        found = symbols.find_symbol(ctx, "", limit=100)
        ids = [card.stable_id for card in found.cards]
        assert ids

        for batch in _batches(ids, 10):
            assert symbols.expand_symbols(ctx, batch).unresolved == ()

    def test_printed_ids_parse_into_a_file_that_exists(self, repo, extractor, counter):
        ctx = context_for(repo)
        result = MapService(extractor, counter).build(ctx, MapRequest(budget=8_000))
        for map_file in result.files:
            for entry in map_file.entries:
                parsed = parse_stable_id(entry.stable_id)
                assert (ctx.root / parsed.path).is_file()
                assert parsed.path == map_file.path


def test_the_expand_limit_reports_the_ids_it_did_not_expand(extractor, counter):
    ctx = context_for("repo_py")
    result = MapService(extractor, counter).build(ctx, MapRequest(budget=8_000))
    ids = [entry.stable_id for map_file in result.files for entry in map_file.entries][:12]

    expanded = SymbolService(extractor, Chars4Counter()).expand_symbols(ctx, ids, limit=10)
    assert len(expanded.cards) == 10
    # The ids past the limit collapse to one counted row: naming each of them
    # spends the answer's budget on the refusal instead of on the bodies.
    assert expanded.unresolved == (("(2 ids)", "not expanded: the per-call limit is 10 symbols"),)


class TestFairDegradation:
    """A batch that will not fit is shortened, never silently emptied.

    The defect this pins: with the budget spent first-come-first-served, ten
    ids returned one body and the other nine vanished into a ceiling marker.
    """

    def _ten_ids(self, extractor, counter):
        ctx = context_for("repo_py")
        result = MapService(extractor, counter).build(ctx, MapRequest(budget=8_000))
        ids = [entry.stable_id for map_file in result.files for entry in map_file.entries]
        assert len(ids) >= 10
        return ctx, ids[:10]

    def test_a_generous_budget_returns_every_body_whole(self, extractor, counter):
        ctx, ids = self._ten_ids(extractor, counter)
        result = SymbolService(extractor, Chars4Counter()).expand_symbols(
            ctx, ids, limit=10, budget=EXPAND_BUDGET_TOKENS
        )
        assert [card.stable_id for card in result.cards] == ids
        assert result.shortened == 0
        assert all(card.body_shown == card.body_total for card in result.cards)
        assert "lines shown" not in render_expansion(result)

    def test_a_tight_budget_still_answers_every_requested_id(self, extractor, counter):
        ctx, ids = self._ten_ids(extractor, counter)
        result = SymbolService(extractor, Chars4Counter()).expand_symbols(
            ctx, ids, limit=10, budget=900
        )
        assert [card.stable_id for card in result.cards] == ids
        assert all(card.body for card in result.cards)
        assert result.unresolved == ()

    def test_every_shortened_body_is_marked_and_counted(self, extractor, counter):
        ctx, ids = self._ten_ids(extractor, counter)
        result = SymbolService(extractor, Chars4Counter()).expand_symbols(
            ctx, ids, limit=10, budget=900
        )
        assert result.shortened > 0
        for card in result.cards:
            if card.body_shown < card.body_total:
                assert "lines shown" in card.body
                assert card.as_dict()["body_truncated"]["lines"] == card.body_total

        rendered = render_expansion(result)
        assert f"{result.shortened} of {len(result.cards)} bodies were shortened" in rendered

    def test_no_body_is_cut_while_a_longer_one_is_whole(self, extractor, counter):
        """Max-min fairness, stated as the property a reader can check."""
        ctx, ids = self._ten_ids(extractor, counter)
        result = SymbolService(extractor, Chars4Counter()).expand_symbols(
            ctx, ids, limit=10, budget=900
        )
        cut = [card.body_total for card in result.cards if card.body_shown < card.body_total]
        whole = [card.body_total for card in result.cards if card.body_shown == card.body_total]
        assert cut, "the tight budget cut nothing, so there is nothing to check"
        assert not whole or max(whole) <= min(cut)

    def test_an_id_batch_too_large_to_seat_is_refused_by_count(self, tmp_path, extractor, counter):
        """More ids than one response can carry is answered, not half-answered."""
        wanted = EXPAND_MAX_SEATS + 5
        source = "\n\n\n".join(f"def f_{index}():\n    return {index}" for index in range(wanted))
        (tmp_path / "many.py").write_text(source + "\n", encoding="utf-8")
        ctx = resolve_repo(tmp_path, None)
        ids = [f"py:many.py::f_{index}" for index in range(wanted)]

        result = SymbolService(extractor, counter).expand_symbols(ctx, ids, limit=wanted)

        assert len(result.cards) == EXPAND_MAX_SEATS
        assert len(result.unresolved) == 1
        assert result.unresolved[0][0] == "(5 ids)"
        assert all("retry with at most" in reason for _, reason in result.unresolved)
        assert all(card.body for card in result.cards)


def test_a_batch_of_long_bodies_survives_the_output_ceiling_whole(tmp_path, extractor, counter):
    """The regression this phase exists for, at the size that produced it.

    Ten symbols whose bodies together dwarf the 16k-token ceiling: before the
    fair split, the JSON answer carried one symbol and a ``truncated`` field,
    and the text answer stopped mid-body. Both halves are asserted, because
    the ceiling trims them by different mechanisms.
    """
    body = "\n".join(f"    value_{line} = {line} * 3" for line in range(400))
    source = "\n\n\n".join(f"def handler_{index}():\n{body}" for index in range(10))
    (tmp_path / "handlers.py").write_text(source + "\n", encoding="utf-8")
    ctx = resolve_repo(tmp_path, None)

    ids = [f"py:handlers.py::handler_{index}" for index in range(10)]
    result = SymbolService(extractor, counter).expand_symbols(ctx, ids, limit=10)

    assert [card.stable_id for card in result.cards] == ids
    assert result.unresolved == ()
    assert result.shortened == 10
    assert all(card.body_shown >= 1 for card in result.cards)

    document = json.loads(
        envelope.wrap_json(ctx, result.as_dict(), counter=counter, items_key="symbols")
    )
    assert "truncated" not in document
    assert len(document["symbols"]) == 10

    text = envelope.wrap(ctx, render_expansion(result), counter=counter)
    assert "output truncated at the" not in text
    for stable_id in ids:
        assert stable_id in text


def test_each_of_two_same_named_go_methods_expands_to_its_own_body(tmp_path, extractor):
    """The collision fix, end to end: distinct ids that address distinct bodies."""
    source = (
        "package config\n\n"
        "type ServerInfo struct{ Host string }\n\n"
        "type AWSConf struct{ Region string }\n\n"
        "func (s ServerInfo) Validate() error {\n\treturn errServer\n}\n\n"
        "func (a *AWSConf) Validate() error {\n\treturn errAWS\n}\n"
    )
    (tmp_path / "config.go").write_text(source, encoding="utf-8")
    ctx = resolve_repo(tmp_path, None)

    result = SymbolService(extractor, Chars4Counter()).expand_symbols(
        ctx, ["go:config.go::ServerInfo.Validate", "go:config.go::AWSConf.Validate"]
    )
    assert result.unresolved == ()
    assert [card.stable_id for card in result.cards] == [
        "go:config.go::ServerInfo.Validate",
        "go:config.go::AWSConf.Validate",
    ]
    assert "errServer" in result.cards[0].body
    assert "errAWS" in result.cards[1].body


def test_each_of_two_same_named_python_functions_expands_to_its_own_body(tmp_path, extractor):
    """The language-agnostic backstop, end to end."""
    source = "def handle():\n    return FIRST\n\n\ndef handle():\n    return SECOND\n"
    (tmp_path / "handlers.py").write_text(source, encoding="utf-8")
    ctx = resolve_repo(tmp_path, None)

    result = SymbolService(extractor, Chars4Counter()).expand_symbols(
        ctx, ["py:handlers.py::handle", "py:handlers.py::handle#2"]
    )
    assert result.unresolved == ()
    assert "FIRST" in result.cards[0].body
    assert "SECOND" in result.cards[1].body


def _batches(values, size):
    """Split ``values`` into lists of at most ``size`` entries."""
    return [values[index : index + size] for index in range(0, len(values), size)]
