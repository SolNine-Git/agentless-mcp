"""The packaged agent guide and its section split.

The census tests here are drift alarms, not documentation. The guide is edited
by hand and the section split derives names from its headings, so a heading
that gains a level, loses its parenthesised tool name, or collides with another
would silently change or shadow an addressable section. Pinning the name list,
the level census and the fence-suppression case turns any of those into a test
failure instead of a section an agent asks for and does not get.
"""

import re

import pytest

from agentless_mcp.core import guide
from agentless_mcp.core.guide import GuideDataError

# The addressable sections, in document order, as measured against the guide at
# the time this pin was written. A guide edit that adds or renames a '##' or
# '###' heading is expected to update this list in the same change.
EXPECTED_SECTIONS = (
    "the-canonical-recipe",
    "the-funnel-and-where-it-stops",
    "stable-ids",
    "the-two-surfaces",
    "per-tool-usage",
    "claude-code-specifics",
    "map",
    "tree",
    "skeleton",
    "expand",
    "slice",
    "find-symbol",
    "refs",
    "explain",
    "path",
    "cycles",
    "communities",
    "diagram",
    "html",
    "resolve-locs",
    "capabilities",
    "index",
    "lint",
    "validate-vote",
    "warmup",
    "project-defaults-agentless-mcp-json",
    "token-counting",
    "exit-codes",
)

# Heading level -> count, over the whole guide, fences excluded.
EXPECTED_LEVELS = {1: 1, 2: 8, 3: 20, 4: 4}


@pytest.fixture(autouse=True)
def _uncached_sections():
    """Clear the split cache around every test.

    ``_sections`` is memoised for the process, so a test that patches the
    resource away would otherwise leave either a poisoned or a stale cache
    behind for whatever runs next.
    """
    guide._sections.cache_clear()
    yield
    guide._sections.cache_clear()


class TestPackagedResource:
    def test_the_guide_ships_with_the_package(self):
        """Read through importlib.resources, so an install-only user has it."""
        text = guide.guide_text()
        assert text.startswith("# agentless-mcp: agent usage guide")
        assert len(text) > 10_000

    def test_a_missing_guide_raises_rather_than_returning_nothing(self, monkeypatch):
        """A broken install must not read as a guide with nothing to say."""

        absent = "agent-guide.md"

        def missing(*_args, **_kwargs):
            raise FileNotFoundError(absent)

        monkeypatch.setattr(guide, "_resource_text", missing)
        with pytest.raises(FileNotFoundError):
            guide.section_names()

    def test_an_unreadable_resource_names_the_broken_install(self, monkeypatch):
        """The OSError is converted so the message says which file is absent."""
        resource = guide.resources.files(guide.PACKAGE) / guide.GUIDE_DIRECTORY

        reason = "no such file"

        def refuse(*_args, **_kwargs):
            raise OSError(reason)

        monkeypatch.setattr(type(resource), "read_text", refuse, raising=False)
        with pytest.raises(GuideDataError, match="missing from the agentless_mcp package"):
            guide._resource_text()


class TestSectionCensus:
    def test_the_section_names_are_the_pinned_list_in_document_order(self):
        assert guide.section_names() == EXPECTED_SECTIONS

    def test_section_names_are_unique(self):
        """A collision would shadow one section; the split raises on it."""
        names = guide.section_names()
        assert len(set(names)) == len(names)

    def test_the_heading_level_census_is_unchanged(self):
        lines = guide.guide_text().splitlines()
        levels: dict[int, int] = {}
        for _, level, _ in guide._headings(lines):
            levels[level] = levels.get(level, 0) + 1
        assert levels == EXPECTED_LEVELS

    def test_hashes_inside_a_fenced_block_are_not_headings(self):
        """The receipt example in the guide opens with three '#' lines."""
        lines = guide.guide_text().splitlines()
        naive = sum(1 for line in lines if re.match(r"^ {0,3}#{1,6}\s+", line))
        found = len(guide._headings(lines))
        assert naive - found == 3
        assert "agentless-mcp-receipt" not in guide.section_names()


class TestSectionText:
    def test_a_section_starts_at_its_own_heading(self):
        text = guide.section_text("refs")
        assert text is not None
        assert text.splitlines()[0].startswith("### `refs`")
        assert "Read the top two tiers as callers" in text

    def test_a_parent_section_contains_its_children(self):
        """The overlap is deliberate: the reference and one entry in it."""
        parent = guide.section_text("per-tool-usage")
        child = guide.section_text("refs")
        assert parent is not None
        assert child is not None
        assert child in parent

    def test_an_unknown_name_is_none_rather_than_empty_text(self):
        assert guide.section_text("no-such-section") is None

    def test_every_section_appears_verbatim_in_the_whole_guide(self):
        whole = guide.guide_text()
        for name in guide.section_names():
            text = guide.section_text(name)
            assert text is not None
            assert text in whole, name


class TestFenceRule:
    """CommonMark closing: same character, at least as long, nothing after."""

    def test_a_shorter_run_does_not_close_a_longer_fence(self):
        lines = ["````", "``` ", "# not a heading", "````", "# heading"]
        assert [text for _, _, text in guide._headings(lines)] == ["heading"]

    def test_the_other_fence_character_does_not_close(self):
        lines = ["~~~", "```", "# not a heading", "~~~", "# heading"]
        assert [text for _, _, text in guide._headings(lines)] == ["heading"]

    def test_a_longer_run_closes_a_shorter_fence(self):
        lines = ["```", "# not a heading", "`````", "# heading"]
        assert [text for _, _, text in guide._headings(lines)] == ["heading"]

    def test_an_info_string_does_not_close_a_fence(self):
        lines = ["```", "# not a heading", "```json", "# still not a heading"]
        assert guide._headings(lines) == []
