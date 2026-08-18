"""Tests for interval merging and line rendering."""

from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.slices import line_wrap_content, merge_intervals

TWELVE_LINES = "\n".join(f"line {index}" for index in range(1, 13))

SCOPED_SOURCE = '''\
class Widget:
    """A widget."""

    def render(self) -> str:
        total = 0
        for part in self.parts:
            total += part
        return str(total)
'''


class TestMergeIntervals:
    def test_overlapping_intervals_merge(self):
        assert merge_intervals([(1, 5), (4, 9)]) == [(1, 9)]

    def test_disjoint_intervals_are_sorted_not_merged(self):
        assert merge_intervals([(20, 25), (1, 5)]) == [(1, 5), (20, 25)]

    def test_contained_interval_is_absorbed(self):
        assert merge_intervals([(1, 30), (5, 9)]) == [(1, 30)]

    def test_empty_input(self):
        assert merge_intervals([]) == []

    def test_caller_list_is_not_mutated(self):
        original = [(20, 25), (1, 5)]
        merge_intervals(original)
        assert original == [(20, 25), (1, 5)]


class TestLineWrapContent:
    def test_whole_file_is_numbered_without_markers(self):
        rendered = line_wrap_content("a\nb\nc")
        assert rendered == "1|a\n2|b\n3|c"

    def test_slice_is_marked_at_both_ends(self):
        rendered = line_wrap_content(TWELVE_LINES, [(4, 6)]).splitlines()
        assert rendered[0] == "..."
        assert rendered[1] == "4|line 4"
        assert rendered[-1] == "..."

    def test_slice_starting_at_line_one_has_no_leading_marker(self):
        rendered = line_wrap_content(TWELVE_LINES, [(1, 3)]).splitlines()
        assert rendered[0] == "1|line 1"

    def test_gap_between_intervals_is_marked_once(self):
        rendered = line_wrap_content(TWELVE_LINES, [(1, 2), (5, 12)]).splitlines()
        assert rendered == [
            "1|line 1",
            "2|line 2",
            "...",
            *[f"{n}|line {n}" for n in range(5, 13)],
        ]

    def test_trailing_marker_does_not_follow_the_last_interval_alone(self):
        """Regression: Agentless decided the trailing ``...`` from whatever
        ``max_line`` held after the loop, so an unordered interval list that
        already reached the end of the file still got an elision marker."""
        rendered = line_wrap_content(TWELVE_LINES, [(1, 12), (5, 7)])
        assert not rendered.endswith("...")
        assert rendered.splitlines()[-1] == "12|line 12"

    def test_add_space_format(self):
        assert line_wrap_content("a", add_space=True) == "1| a "

    def test_no_line_number_format(self):
        assert line_wrap_content("a\nb", no_line_number=True) == "a\nb"

    def test_scope_headers_come_from_symbols(self):
        symbols = TreeSitterExtractor().extract_from_source(SCOPED_SOURCE, "python", "widget.py")
        rendered = line_wrap_content(SCOPED_SOURCE, [(7, 7)], symbols=symbols).splitlines()
        assert rendered == [
            "...",
            "1|class Widget:",
            "4|    def render(self) -> str:",
            "...",
            "7|            total += part",
            "...",
        ]

    def test_scope_headers_are_not_repeated_across_intervals(self):
        symbols = TreeSitterExtractor().extract_from_source(SCOPED_SOURCE, "python", "widget.py")
        rendered = line_wrap_content(SCOPED_SOURCE, [(6, 6), (8, 8)], symbols=symbols)
        assert rendered.count("1|class Widget:") == 1
        assert rendered.count("4|    def render(self) -> str:") == 1
