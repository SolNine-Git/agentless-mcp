"""Tests for interval merging, line counting and line rendering."""

from dataclasses import replace

import pytest

from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.slices import line_count, line_wrap_content, merge_intervals
from agentless_mcp.util.errors import AgentlessError

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

    def test_the_removed_prompt_options_left_one_render_path(self):
        """``add_space`` and ``no_line_number`` were Agentless prompt knobs
        that no caller here ever set, and the dataclass existed only to route
        between them. One spelling is left. It is not yet ``line_prefix``'s:
        see this module's docstring."""
        assert line_wrap_content("a") == "1|a"
        with pytest.raises(TypeError):
            line_wrap_content("a", add_space=True)

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

    def test_a_header_the_render_already_showed_as_content_is_not_repeated(self):
        """Regression: ``shown_scopes`` recorded only the headers this render
        wrote, so an interval covering the class line and a later interval
        inside that class printed the class line twice and the numbers ran
        backwards."""
        symbols = TreeSitterExtractor().extract_from_source(SCOPED_SOURCE, "python", "widget.py")
        rendered = line_wrap_content(SCOPED_SOURCE, [(1, 2), (7, 7)], symbols=symbols).splitlines()
        assert rendered == [
            "1|class Widget:",
            '2|    """A widget."""',
            "...",
            "4|    def render(self) -> str:",
            "...",
            "7|            total += part",
            "...",
        ]


class TestLineCount:
    """A trailing newline terminates the last line; it does not add one."""

    def test_a_newline_terminated_file_counts_its_real_lines(self):
        assert line_count("a\nb\n") == 2

    def test_a_file_without_a_final_newline_counts_the_same(self):
        assert line_count("a\nb") == 2

    def test_a_blank_last_line_is_still_a_line(self):
        assert line_count("a\n\n") == 2

    def test_an_empty_file_has_no_lines(self):
        assert line_count("") == 0

    def test_the_whole_file_render_has_no_phantom_last_line(self):
        assert line_wrap_content("a\nb\n") == "1|a\n2|b"


class TestAnUnsatisfiableIntervalIsRefused:
    """ "Nothing was asked for" and "what was asked for cannot be answered"
    are opposite requests. Answering the second one with the whole file is
    both the token blow-up the slice API exists to prevent and a false claim
    about what the returned lines are."""

    def test_no_interval_still_means_the_whole_file(self):
        assert line_wrap_content(TWELVE_LINES) == line_wrap_content(TWELVE_LINES, [])

    def test_a_transposed_interval_is_refused(self):
        with pytest.raises(AgentlessError, match="no requested line range falls inside"):
            line_wrap_content(TWELVE_LINES, [(60, 30)])

    def test_a_negative_interval_is_not_read_as_the_whole_file(self):
        with pytest.raises(AgentlessError, match="no requested line range falls inside"):
            line_wrap_content(TWELVE_LINES, [(-9, -1)])

    def test_an_interval_past_the_end_is_refused(self):
        with pytest.raises(AgentlessError, match="no requested line range falls inside"):
            line_wrap_content(TWELVE_LINES, [(30, 40)])

    def test_one_satisfiable_interval_is_enough_to_answer(self):
        rendered = line_wrap_content(TWELVE_LINES, [(2, 3), (30, 40)]).splitlines()
        assert rendered == ["...", "2|line 2", "3|line 3", "..."]


class TestASymbolWithNoEndLine:
    """An end line of ``None`` means one line, here and in `core.locs` alike.

    It arrives only from a cache row an older build wrote. Reading it as "this
    symbol encloses everything after it" made one stale row a scope header
    stuck above every later slice of the file.
    """

    def test_it_does_not_become_a_header_for_the_rest_of_the_file(self):
        symbols = TreeSitterExtractor().extract_from_source(SCOPED_SOURCE, "python", "widget.py")
        stale = [
            replace(symbol, end_line_number=None) if symbol.name == "Widget" else symbol
            for symbol in symbols
        ]
        rendered = line_wrap_content(SCOPED_SOURCE, [(7, 7)], symbols=stale)
        assert "1|class Widget:" not in rendered
        assert "4|    def render(self) -> str:" in rendered
