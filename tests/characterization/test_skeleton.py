"""Byte-exact skeleton goldens for the committed fixture repositories.

The goldens are the contract: a grammar bump or a refactor that changes what
a skeleton looks like has to change a checked-in file, in a diff a reviewer
sees. Regenerate deliberately, never reflexively:

    uv run python -c "
    from pathlib import Path
    from tests.characterization.test_skeleton import regenerate
    regenerate()"

Compression is asserted as a band, not a number. Research puts the useful
range around 5-7x (SWEzze 2603.28119: ~6x beats full context, 22-50x is
harmful), so a fixture that drifts under 3x or over 9x means the skeletonizer
or the fixture stopped being representative.
"""

from pathlib import Path

import pytest

from agentless_mcp.core.skeleton import DOCSTRING_MAX_CHARS, compression_ratio, skeletonize

FIXTURES = Path(__file__).parent / "fixtures"
GOLDENS = Path(__file__).parent / "goldens"

REPOS = (
    ("repo_py", "python", "*.py"),
    ("repo_ts", "typescript", "*.ts"),
    ("repo_go", "go", "*.go"),
)

MIN_RATIO = 3.0
MAX_RATIO = 9.0


def _cases():
    """Return (repo, language, source path) for every fixture file.

    A list, not a generator: a class-level parametrize is evaluated once per
    test method, and a generator would hand the second method an empty set --
    a silently skipped suite rather than a failing one.
    """
    return [
        pytest.param(repo, language, path, id=f"{repo}/{path.name}")
        for repo, language, pattern in REPOS
        for path in sorted((FIXTURES / repo).glob(pattern))
    ]


def golden_path(repo: str, source: Path) -> Path:
    """Where the golden for one fixture file lives."""
    return GOLDENS / repo / f"{source.name}.skel"


def regenerate() -> None:
    """Rewrite every golden from the current skeletonizer."""
    for repo, language, pattern in REPOS:
        for source in sorted((FIXTURES / repo).glob(pattern)):
            target = golden_path(repo, source)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(
                skeletonize(source.read_text(encoding="utf-8"), language),
                encoding="utf-8",
            )


@pytest.mark.parametrize(("repo", "language", "source"), _cases())
class TestSkeletonGoldens:
    def test_matches_golden(self, repo, language, source):
        produced = skeletonize(source.read_text(encoding="utf-8"), language)
        assert produced == golden_path(repo, source).read_text(encoding="utf-8")

    def test_compression_ratio_in_band(self, repo, language, source):
        text = source.read_text(encoding="utf-8")
        ratio = compression_ratio(text, skeletonize(text, language))
        assert MIN_RATIO <= ratio <= MAX_RATIO, f"{source.name} compresses {ratio:.2f}x"

    def test_fixture_is_large_enough_to_be_meaningful(self, repo, language, source):
        assert len(source.read_text(encoding="utf-8").splitlines()) >= 40


class TestSkeletonSemantics:
    def test_imports_are_kept(self):
        source = (FIXTURES / "repo_py" / "reports.py").read_text(encoding="utf-8")
        skeleton = skeletonize(source, "python")
        assert "from decimal import Decimal" in skeleton
        assert "from pricing import PriceBook, Tier, format_money" in skeleton

    def test_function_bodies_are_elided(self):
        source = (FIXTURES / "repo_py" / "reports.py").read_text(encoding="utf-8")
        skeleton = skeletonize(source, "python")
        assert "def _format_row(item: Item, line_total: Decimal) -> str:\n    ...\n" in skeleton
        assert "item.sku.ljust(16)" not in skeleton

    def test_module_and_class_constants_are_kept(self):
        source = (FIXTURES / "repo_py" / "reports.py").read_text(encoding="utf-8")
        skeleton = skeletonize(source, "python")
        assert "REPORT_WIDTH = 72" in skeleton

    def test_docstrings_and_comments_are_stripped_by_default(self):
        source = (FIXTURES / "repo_py" / "inventory.py").read_text(encoding="utf-8")
        skeleton = skeletonize(source, "python")
        assert '"""' not in skeleton
        assert "# Reorder thresholds" not in skeleton

    def test_docstrings_on_truncates_at_the_cap(self):
        source = (FIXTURES / "repo_py" / "inventory.py").read_text(encoding="utf-8")
        skeleton = skeletonize(source, "python", docstrings=True)

        docstring_lines = [line.strip() for line in skeleton.splitlines() if '"""' in line]
        assert docstring_lines, "expected docstrings to survive"
        for line in docstring_lines:
            assert len(line.strip('"')) <= DOCSTRING_MAX_CHARS

        truncated = [line for line in docstring_lines if line.endswith('..."""')]
        assert truncated, "expected the long dump_inventory docstring to be truncated"
        assert "A single stocked item." in skeleton

    def test_comments_are_stripped_even_with_docstrings_on(self):
        source = (FIXTURES / "repo_py" / "inventory.py").read_text(encoding="utf-8")
        skeleton = skeletonize(source, "python", docstrings=True)
        assert "# Reorder thresholds" not in skeleton

    def test_number_lines_preserves_original_numbering(self):
        source = "import os\n\n\ndef f():\n    return os.getcwd()\n"
        skeleton = skeletonize(source, "python", number_lines=True)
        assert skeleton.splitlines()[0] == "1| import os"
        assert "4| def f():" in skeleton
        assert "5|     ..." in skeleton

    def test_go_bodies_keep_their_braces(self):
        source = (FIXTURES / "repo_go" / "pricing.go").read_text(encoding="utf-8")
        skeleton = skeletonize(source, "go")
        assert "func FormatMoney(amount float64) string { ... }" in skeleton
        assert "math.Round" not in skeleton

    def test_typescript_class_members_survive(self):
        source = (FIXTURES / "repo_ts" / "pricing.ts").read_text(encoding="utf-8")
        skeleton = skeletonize(source, "typescript")
        assert "export class PriceBook {" in skeleton
        assert "private readonly costs: Map<string, number>;" in skeleton
        assert "quote(sku: string, tier: Tier, quantity = 1): number { ... }" in skeleton
