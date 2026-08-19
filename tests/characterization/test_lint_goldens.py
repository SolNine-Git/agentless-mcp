"""Byte-exact goldens for the patch-lint report.

Same mechanism as the map and graph goldens, and the same reason. What a lint
report *says* is the whole product here -- the severity a check reports at, the
order findings come back in, the wording that tells a reader whether something
did not happen or happened and was clean -- and none of that is visible in a
unit test asserting one substring. A change to any of it has to show up in a
diff a reviewer sees.

The two candidates below are deliberately built to trip a different check
each, against the committed Python fixture repository.

Regenerate deliberately, never reflexively:

    uv run python -c "
    from tests.characterization.test_lint_goldens import regenerate
    regenerate()"
"""

from pathlib import Path

from agentless_mcp.application import envelope, render
from agentless_mcp.application.lint_service import LintCandidateInput, LintService
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.tokens import Chars4Counter

FIXTURES = Path(__file__).parent / "fixtures"
GOLDENS = Path(__file__).parent / "goldens" / "lint"

PLACEHOLDER = "<REPO>"

# Two candidates, so that the report's per-candidate shape is pinned as well
# as its findings. The first renames a function two other files still call;
# the second calls a helper that does not exist under a name close to one that
# does, and hands an existing helper an argument too many.
RENAMING_CANDIDATE = '''\
### pricing.py
<<<<<<< SEARCH
def format_money(amount: Decimal) -> str:
    """Render an amount with the configured currency suffix."""
    quantized = amount.quantize(CENTS, rounding=ROUND_HALF_UP)
    return f"{quantized} {CURRENCY}"
=======
def render_money(amount: Decimal) -> str:
    """Render an amount with the configured currency suffix."""
    quantized = amount.quantize(CENTS, rounding=ROUND_HALF_UP)
    return f"{quantized} {CURRENCY}"
>>>>>>> REPLACE
'''

CALLING_CANDIDATE = """\
### inventory.py
<<<<<<< SEARCH
    def reorder_list(self) -> list[Item]:
=======
    def reorder_list(self) -> list[Item]:
        _probe = reorder_lst()
>>>>>>> REPLACE

### reports.py
<<<<<<< SEARCH
    money = format_money(line_total)
=======
    money = format_money(line_total, Tier.TRADE)
>>>>>>> REPLACE
"""


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


def build_outputs() -> dict[str, str]:
    """Produce the lint goldens for the Python fixture repository."""
    counter = Chars4Counter()
    ctx = context_for("repo_py")
    report = LintService(TreeSitterExtractor()).lint(
        ctx,
        [
            LintCandidateInput(id="01-renames", text=RENAMING_CANDIDATE),
            LintCandidateInput(id="02-calls", text=CALLING_CANDIDATE),
        ],
    )
    return {
        "lint.txt": normalise(envelope.wrap(ctx, render.render_lint(report), counter=counter), ctx),
        "lint.json": normalise(
            envelope.wrap_json(ctx, report.as_dict(), counter=counter, items_key="candidates"),
            ctx,
        ),
    }


def golden_path(name: str) -> Path:
    """Where one golden lives."""
    return GOLDENS / f"repo_py.{name}"


def regenerate() -> None:
    """Rewrite every lint golden from the current renderers."""
    GOLDENS.mkdir(parents=True, exist_ok=True)
    for name, text in build_outputs().items():
        golden_path(name).write_text(text, encoding="utf-8")


def test_the_rendered_report_matches_its_golden():
    produced = build_outputs()["lint.txt"]
    assert produced == golden_path("lint.txt").read_text(encoding="utf-8")


def test_the_json_form_matches_its_golden():
    produced = build_outputs()["lint.json"]
    assert produced == golden_path("lint.json").read_text(encoding="utf-8")


def test_two_runs_over_one_tree_are_byte_identical():
    assert build_outputs() == build_outputs()


def test_the_seeded_patch_trips_every_resolution_dependent_check():
    text = build_outputs()["lint.txt"]
    for check in ("dangling_references", "dangling_callers", "arity"):
        assert check in text, check
