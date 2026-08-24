"""Byte-exact map and tree goldens, plus the token-budget regression pins.

The goldens cover the whole envelope, receipt included, not just the rendered
body: the receipt format is part of the contract an agent reads, so a change
to it has to show up in a diff a reviewer sees. Two things are normalised to
keep that hermetic -- the repository's absolute path becomes ``<REPO>``, and
the git state comes from a pinned context rather than from whatever working
tree the suite happens to run inside.

Regenerate deliberately, never reflexively:

    uv run python -c "
    from tests.characterization.test_map_goldens import regenerate
    regenerate()"

The token pins are the second half of the contract. A map that silently
doubles in size still passes a byte-exact golden the moment someone
regenerates it; the pins say the size itself is a decision. The band is +/-5%,
and the assertion that the body fits its own budget is what proves the packing
search is doing its job.
"""

import json
from pathlib import Path

import pytest

from agentless_mcp.application import envelope
from agentless_mcp.application.map_service import MapRequest, MapService
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.symbols import SIGNATURE_MAX_CHARS
from agentless_mcp.util.tokens import Chars4Counter

FIXTURES = Path(__file__).parent / "fixtures"
GOLDENS = Path(__file__).parent / "goldens" / "map"

REPOS = ("repo_py", "repo_ts", "repo_go", "repo_py_tests")

# Measured 2026-08-18 with the chars/4 estimator on the committed fixtures.
# A drift outside +/-5% means the map's shape changed; decide whether that was
# intended before touching these numbers.
#
# repo_go re-pinned 357 -> 388 on 2026-08-19 (Phase 8), deliberately: Go
# methods now carry their receiver type, so each method's id grew by the
# receiver name and each method line gained the one-level indent every other
# language's methods already had. That is 8.7% of this fixture's map and it
# buys stable-id uniqueness -- `Validate` on four receivers used to be one id
# four times. Nothing else in the map's shape moved.
TOKEN_PINS = {
    "repo_py": 711,
    "repo_ts": 630,
    # 388 before stage 6c. Go type declarations became symbols of their own
    # there, so this repository gained six: the receiver type every method
    # names now has a line the map can point at.
    "repo_go": 449,
    # Pinned 2026-08-23 with the companion section. This repository holds
    # twelve files against a ten-file map on purpose: the two test files fall
    # outside the ranking, which is the only state in which the companion
    # section has anything to say. The other three fixtures fit inside the
    # limit entirely, so every file is ranked and no test is ever left over
    # to be a companion -- which is why they pin nothing about this feature
    # and this repository had to exist.
    "repo_py_tests": 1185,
}
TOKEN_TOLERANCE = 0.05

PLACEHOLDER = "<REPO>"


def context_for(repo: str) -> RepoContext:
    """A pinned context so the receipt does not depend on the working tree."""
    return RepoContext(
        root=(FIXTURES / repo).resolve(),
        head_sha="0000000f",
        tree_oid="1111111f",
        dirty_count=0,
        note="",
    )


def normalise(text: str, ctx: RepoContext) -> str:
    """Replace the absolute repository path with a stable placeholder."""
    return text.replace(str(ctx.root), PLACEHOLDER)


def build_outputs(repo: str) -> dict[str, str]:
    """Produce every golden for one fixture repository."""
    extractor = TreeSitterExtractor()
    counter = Chars4Counter()
    ctx = context_for(repo)

    maps = MapService(extractor, counter)
    views = ViewService(extractor)

    result = maps.build(ctx, MapRequest())
    tree = views.tree(ctx)

    return {
        "map.txt": normalise(
            envelope.wrap(
                ctx,
                maps.render_text(result),
                counter=counter,
                truncation=envelope.Truncation(
                    shown=result.included, total=result.candidates, unit="symbols"
                ),
            ),
            ctx,
        ),
        "map.json": normalise(
            envelope.wrap_json(ctx, result.as_dict(), counter=counter, items_key="files"), ctx
        ),
        "tree.txt": normalise(envelope.wrap(ctx, tree.text, counter=counter), ctx),
    }


def golden_path(repo: str, name: str) -> Path:
    """Where one golden lives."""
    return GOLDENS / f"{repo}.{name}"


def regenerate() -> None:
    """Rewrite every map and tree golden from the current renderers."""
    GOLDENS.mkdir(parents=True, exist_ok=True)
    for repo in REPOS:
        for name, text in build_outputs(repo).items():
            golden_path(repo, name).write_text(text, encoding="utf-8")


@pytest.mark.parametrize("repo", REPOS)
class TestGoldens:
    def test_map_text_matches_golden(self, repo):
        produced = build_outputs(repo)["map.txt"]
        assert produced == golden_path(repo, "map.txt").read_text(encoding="utf-8")

    def test_map_json_matches_golden(self, repo):
        produced = build_outputs(repo)["map.json"]
        assert produced == golden_path(repo, "map.json").read_text(encoding="utf-8")

    def test_tree_matches_golden(self, repo):
        produced = build_outputs(repo)["tree.txt"]
        assert produced == golden_path(repo, "tree.txt").read_text(encoding="utf-8")

    def test_the_json_and_text_renders_agree_on_what_was_included(self, repo):
        outputs = build_outputs(repo)
        document = json.loads(outputs["map.json"])
        printed = [line for line in outputs["map.txt"].splitlines() if "  [" in line]
        assert len(printed) == document["symbols_included"]

    def test_the_two_renders_agree_on_the_test_companions(self, repo):
        """The companion section is a second listing, so it needs its own check.

        The symbol check above counts locator rows and a companion row carries
        no locator, so it would pass while the two forms disagreed. This is
        the same cross-check for the section the ranking cannot produce:
        every companion in the JSON is a row in the text, spelled the way the
        answer contract wants it, and neither form carries one the other does
        not.
        """
        outputs = build_outputs(repo)
        companions = json.loads(outputs["map.json"])["test_companions"]
        rows = [line for line in outputs["map.txt"].splitlines() if line.startswith("  tests/")]

        assert len(rows) == len(companions["rows"])
        for row, listed in zip(rows, companions["rows"], strict=True):
            assert row.startswith(f"  {listed['path']}:{listed['start']}-{listed['end']}")
        assert companions["omitted"] == companions["total"] - len(companions["rows"])

    def test_every_symbol_occupies_exactly_one_line(self, repo):
        """One symbol per line is the map's format contract.

        A signature taken verbatim from a multi-line declaration spreads one
        symbol across six lines and spends the budget doing it, so the
        invariant is asserted on the rendered golden rather than only on the
        value object that enforces it.
        """
        document = json.loads(build_outputs(repo)["map.json"])
        for map_file in document["files"]:
            for entry in map_file["symbols"]:
                assert "\n" not in entry["signature"]
                assert len(entry["signature"]) <= SIGNATURE_MAX_CHARS


@pytest.mark.parametrize("repo", REPOS)
class TestTokenBudget:
    def test_the_map_body_fits_its_own_budget(self, repo, counter, extractor):
        maps = MapService(extractor, counter)
        ctx = context_for(repo)
        result = maps.build(ctx, MapRequest())
        assert counter.count(maps.render_text(result)) <= result.budget

    def test_the_map_size_is_pinned_within_five_percent(self, repo, counter, extractor):
        maps = MapService(extractor, counter)
        measured = counter.count(maps.render_text(maps.build(context_for(repo), MapRequest())))
        pinned = TOKEN_PINS[repo]
        drift = abs(measured - pinned) / pinned
        assert drift <= TOKEN_TOLERANCE, f"{repo}: {measured} tokens, pinned {pinned}"

    def test_a_tighter_budget_shows_fewer_symbols(self, repo, counter, extractor):
        maps = MapService(extractor, counter)
        ctx = context_for(repo)
        generous = maps.build(ctx, MapRequest(budget=8_000))
        tight = maps.build(ctx, MapRequest(budget=200))
        assert tight.included < generous.included
        assert tight.candidates == generous.candidates
