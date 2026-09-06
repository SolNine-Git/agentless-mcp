"""The git argv history asks with, and the parser that reads -z log output."""

import pytest

from agentless_mcp.core import githistory
from agentless_mcp.util.errors import OperationFailed


class TestLogArguments:
    def test_the_argv_pins_the_span_the_format_and_the_count(self):
        assert githistory.log_arguments("src/app.py", 12, 40, max_count=11) == [
            "log",
            "-L12,40:src/app.py",
            "--no-patch",
            "-z",
            f"--format={githistory.HISTORY_FORMAT}",
            "--max-count=11",
            "--",
        ]

    def test_a_path_holding_a_colon_rides_inside_the_range_token(self):
        argv = githistory.log_arguments("odd:name.py", 1, 1, max_count=1)
        assert argv[1] == "-L1,1:odd:name.py"
        assert argv[-1] == "--"

    @pytest.mark.parametrize(("start", "end", "count"), [(0, 1, 1), (5, 4, 1), (1, 1, 0)])
    def test_an_impossible_span_or_count_is_refused(self, start, end, count):
        with pytest.raises(OperationFailed):
            githistory.log_arguments("a.py", start, end, max_count=count)

    def test_the_dirty_check_defeats_pathspec_magic(self):
        assert githistory.diff_arguments(":!x.py") == ["diff", "--quiet", "HEAD", "--", "./:!x.py"]


def record(sha, authored, subject, body):
    """One commit as git -z prints it: four NUL-terminated fields."""
    return "\x00".join((sha, authored, subject, body)) + "\x00"


class TestParseLog:
    def test_two_records_come_back_in_order_with_their_bodies(self):
        text = record("a" * 40, "2026-08-23T16:43:57-04:00", "fix: sink", "why\n\nmore\n")
        text += record("b" * 40, "2026-08-19T22:34:50-04:00", "feat", "")
        records = githistory.parse_log(text, capped=False)

        assert [entry.subject for entry in records] == ["fix: sink", "feat"]
        assert records[0].body == "why\n\nmore\n"
        assert records[1].body == ""

    def test_literal_format_escapes_in_a_body_are_ordinary_text(self):
        body = "the body says %x00 and \\x00 in words\n"
        records = githistory.parse_log(record("a" * 40, "d", "s", body), capped=False)
        assert records[0].body == body

    def test_control_characters_and_a_lone_surrogate_pass_through(self):
        body = "cr\r here \x1b[0m u2028\u2028 sur\udcff end"
        records = githistory.parse_log(record("a" * 40, "d", "s", body), capped=False)
        assert records[0].body == body

    def test_a_trailing_partial_record_is_dropped_when_the_cap_cut_it(self):
        text = record("a" * 40, "d", "s", "b") + "c" * 40 + "\x00date"
        records = githistory.parse_log(text, capped=True)
        assert len(records) == 1

    def test_a_trailing_partial_record_is_refused_when_nothing_cut_it(self):
        text = record("a" * 40, "d", "s", "b") + "c" * 40 + "\x00date"
        with pytest.raises(OperationFailed, match="history cannot read"):
            githistory.parse_log(text, capped=False)

    def test_patch_text_after_a_record_terminator_is_refused(self):
        """A git that ignores --no-patch under -L prints the diff into the next sha field."""
        text = record("a" * 40, "d", "s", "b") + "diff --git a/x.py b/x.py\n@@ -1 +1 @@\n"
        text += record("b" * 40, "d", "s", "b")
        with pytest.raises(OperationFailed, match="history cannot read"):
            githistory.parse_log(text, capped=False)

    def test_one_nul_inside_a_body_is_refused_on_the_field_count(self):
        text = record("a" * 40, "d", "s", "before\x00after") + record("b" * 40, "d", "s", "")
        with pytest.raises(OperationFailed, match="history cannot read"):
            githistory.parse_log(text, capped=False)

    def test_four_nuls_inside_a_body_are_refused_rather_than_read_as_a_sha(self):
        """Four extra NULs keep the count a multiple of four, so only the sha check can catch it."""
        text = record("a" * 40, "d", "s", "\x00".join(("w", "x", "y", "z", "end")))
        with pytest.raises(OperationFailed, match="40-character sha"):
            githistory.parse_log(text, capped=False)

    def test_a_record_without_an_author_date_is_refused(self):
        with pytest.raises(OperationFailed, match="no author date"):
            githistory.parse_log(record("a" * 40, "", "s", "b"), capped=False)

    @pytest.mark.parametrize("sha", ["A" * 40, "a" * 39, "a" * 41, "g" * 40, ""])
    def test_a_field_that_is_not_a_sha_is_refused(self, sha):
        with pytest.raises(OperationFailed, match="40-character sha"):
            githistory.parse_log(record(sha, "d", "s", "b"), capped=False)

    def test_a_complete_record_is_validated_even_when_the_cap_cut_the_stream(self):
        with pytest.raises(OperationFailed, match="40-character sha"):
            githistory.parse_log(record("nope", "d", "s", "b"), capped=True)

    def test_empty_output_is_no_records(self):
        assert githistory.parse_log("", capped=False) == ()


class TestClassifyFailure:
    @pytest.mark.parametrize(
        ("note", "expected"),
        [
            (
                "git log exited 128: fatal: There is no path a.py in the commit",
                githistory.HistoryFailure.NO_PATH,
            ),
            (
                "git log exited 128: fatal: file a.py has only 3 lines",
                githistory.HistoryFailure.SPAN_BEYOND_HEAD,
            ),
            ("git log timed out after 30.0s", githistory.HistoryFailure.OTHER),
            (
                "git is not installed, so repository state is unknown",
                githistory.HistoryFailure.NO_GIT,
            ),
            ("git log could not be run: Permission denied", githistory.HistoryFailure.NO_GIT),
            ("git log exited 129: usage", githistory.HistoryFailure.OTHER),
        ],
    )
    def test_each_note_maps_to_its_failure(self, note, expected):
        assert githistory.classify_failure(note) is expected
