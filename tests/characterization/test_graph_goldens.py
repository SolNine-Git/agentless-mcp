"""Byte-exact goldens for the resolved-graph views.

Same mechanism and the same reasoning as the map goldens: the whole envelope
is captured, the repository path is normalised to ``<REPO>``, and the git
state comes from a pinned context so the output does not depend on the working
tree the suite runs inside.

What these pin that the unit tests do not is the *rendering* -- which tier a
row lands in, in which order, and how a bounded section announces what it left
out. A change to any of that has to show up in a diff a reviewer sees.

Regenerate deliberately, never reflexively:

    uv run python -c "
    from tests.characterization.test_graph_goldens import regenerate
    regenerate()"
"""

import json
from pathlib import Path

import pytest

from agentless_mcp.application import envelope, render
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.tokens import Chars4Counter

FIXTURES = Path(__file__).parent / "fixtures"
GOLDENS = Path(__file__).parent / "goldens" / "graph"

PLACEHOLDER = "<REPO>"

# One case per repository: the symbol explained, and the pair a path is traced
# between. Chosen to exercise the precise tiers the default path accepts; weak
# name-only tiers have separate opt-in unit coverage.
CASES = {
    "repo_py": ("reorder_report", "reorder_report", "format_money"),
    "repo_ts": ("reorderReport", "reorderReport", "formatMoney"),
}

REPOS = tuple(CASES)


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
    """Produce every graph golden for one fixture repository."""
    counter = Chars4Counter()
    graphs = GraphService(TreeSitterExtractor())
    ctx = context_for(repo)
    target, source_symbol, target_symbol = CASES[repo]

    explained = graphs.explain(ctx, target)
    traced = graphs.path(ctx, source_symbol, target_symbol)
    cycles = graphs.cycles(ctx)
    grouped = graphs.communities(ctx)
    drawn = graphs.diagram(ctx, group_by_communities=True)

    return {
        "explain.txt": normalise(
            envelope.wrap(ctx, render.render_explanation(explained), counter=counter), ctx
        ),
        "explain.json": normalise(
            envelope.wrap_json(ctx, explained.as_dict(), counter=counter, items_key="fan_in"), ctx
        ),
        "path.txt": normalise(envelope.wrap(ctx, render.render_path(traced), counter=counter), ctx),
        "cycles.txt": normalise(
            envelope.wrap(ctx, render.render_cycles(cycles), counter=counter), ctx
        ),
        "communities.txt": normalise(
            envelope.wrap(ctx, render.render_communities(grouped), counter=counter), ctx
        ),
        # The diagram alone travels without an envelope, because that is how
        # both adapters emit it: the CLI writes it into a document and the MCP
        # tool fences it into a body. A receipt in front of it would be text
        # nobody could paste anywhere.
        "diagram.mmd": normalise(drawn.text, ctx),
    }


def golden_path(repo: str, name: str) -> Path:
    """Where one golden lives."""
    return GOLDENS / f"{repo}.{name}"


def regenerate() -> None:
    """Rewrite every graph golden from the current renderers."""
    GOLDENS.mkdir(parents=True, exist_ok=True)
    for repo in REPOS:
        for name, text in build_outputs(repo).items():
            golden_path(repo, name).write_text(text, encoding="utf-8")


GOLDEN_NAMES = (
    "explain.txt",
    "explain.json",
    "path.txt",
    "cycles.txt",
    "communities.txt",
    "diagram.mmd",
)


@pytest.mark.parametrize("repo", REPOS)
@pytest.mark.parametrize("name", GOLDEN_NAMES)
def test_the_rendered_view_matches_its_golden(repo, name):
    produced = build_outputs(repo)[name]
    assert produced == golden_path(repo, name).read_text(encoding="utf-8")


@pytest.mark.parametrize("repo", REPOS)
class TestContract:
    def test_the_json_and_text_renders_agree_on_the_symbol(self, repo):
        outputs = build_outputs(repo)
        document = json.loads(outputs["explain.json"])
        assert document["symbol"]["stable_id"] in outputs["explain.txt"]

    def test_every_fan_row_carries_a_tier_the_resolver_defines(self, repo):
        document = json.loads(build_outputs(repo)["explain.json"])
        tiers = {group["tier"] for group in (*document["fan_in"], *document["fan_out"])}
        assert tiers <= {"same_file", "imported", "unique", "ambiguous"}

    def test_two_renders_of_one_tree_are_byte_identical(self, repo):
        assert build_outputs(repo) == build_outputs(repo)
