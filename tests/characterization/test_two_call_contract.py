"""The two-call contract: every id a map prints must expand.

This is the single property the recommended recipe rests on. If `map` prints
an id that `expand_symbols` cannot resolve, the escalation step fails at
exactly the moment an agent has committed to it, and the failure looks like
"the symbol does not exist" rather than "the id format drifted". Asserted per
fixture repository and across every language, because the id is built from
the extractor's own language and path fields and each handler fills those in
separately.
"""

from pathlib import Path

import pytest

from agentless_mcp.application.map_service import MapRequest, MapService
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.core.symbols import parse_stable_id

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

        symbols = SymbolService(extractor)
        for batch in _batches(ids, 10):
            expanded = symbols.expand_symbols(ctx, batch)
            assert expanded.unresolved == (), f"{repo}: {expanded.unresolved}"
            assert len(expanded.cards) == len(batch)

    def test_every_expanded_body_carries_its_own_first_line(self, repo, extractor, counter):
        ctx = context_for(repo)
        result = MapService(extractor, counter).build(ctx, MapRequest(budget=8_000))
        first = next(entry for map_file in result.files for entry in map_file.entries)

        card = SymbolService(extractor).expand_symbols(ctx, [first.stable_id]).cards[0]
        assert card.body.startswith(f"{first.line}| ")
        assert card.stable_id == first.stable_id

    def test_every_id_the_symbol_search_prints_expands(self, repo, extractor):
        ctx = context_for(repo)
        symbols = SymbolService(extractor)
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

    expanded = SymbolService(extractor).expand_symbols(ctx, ids, limit=10)
    assert len(expanded.cards) == 10
    assert len(expanded.unresolved) == 2
    assert all("per-call limit" in reason for _, reason in expanded.unresolved)


def _batches(values, size):
    """Split ``values`` into lists of at most ``size`` entries."""
    return [values[index : index + size] for index in range(0, len(values), size)]
