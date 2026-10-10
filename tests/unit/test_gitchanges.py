"""Review mode's git argv and the parsers that read git's change lists."""

import pytest

from agentless_mcp.core import gitchanges
from agentless_mcp.core.gitchanges import Change, Side
from agentless_mcp.util.errors import OperationFailed

SHA = "08ec32abb9b149de9d10310c24bd12bd1fe8d874"


def record(old_mode, new_mode, status, *paths):
    """One `git diff --raw -z` record."""
    meta = f":{old_mode} {new_mode} b1bfde2 0000000 {status}".encode()
    return b"\0".join([meta, *(path.encode() for path in paths)]) + b"\0"


class TestTheArgv:
    def test_the_revision_follows_end_of_options(self):
        assert gitchanges.revision_arguments("--output=x") == [
            "rev-parse",
            "--verify",
            "--end-of-options",
            "--output=x^{commit}",
        ]

    def test_the_diff_lists_raw_records_relative_to_the_root(self):
        assert gitchanges.changes_arguments(SHA) == [
            "diff",
            "--raw",
            "-z",
            "-M",
            "--relative",
            SHA,
            "--",
        ]


class TestParseCommit:
    @pytest.mark.parametrize("name", [SHA, "a" * 64])
    def test_a_full_object_name_is_kept(self, name):
        assert gitchanges.parse_commit(f"{name}\n") == name

    @pytest.mark.parametrize("text", ["", SHA[:8], f"{SHA}\n{SHA}", "not a sha"])
    def test_anything_else_is_refused(self, text):
        with pytest.raises(OperationFailed, match="did not print one full object name"):
            gitchanges.parse_commit(text)


class TestParseChanges:
    @pytest.mark.parametrize(
        ("raw", "expected"),
        [
            (
                record("100644", "100644", "M", "a.py"),
                Change("M", "a.py", "a.py", Side.FILE, Side.FILE),
            ),
            (
                record("000000", "100755", "A", "run.py"),
                Change("A", "run.py", "run.py", Side.ABSENT, Side.FILE),
            ),
            (
                record("100644", "000000", "D", "gone.py"),
                Change("D", "gone.py", "gone.py", Side.FILE, Side.ABSENT),
            ),
            (
                record("100644", "100644", "R086", "old.py", "new.py"),
                Change("R", "new.py", "old.py", Side.FILE, Side.FILE),
            ),
            (
                record("100644", "100644", "C075", "src.py", "copy.py"),
                Change("C", "copy.py", "src.py", Side.ABSENT, Side.FILE),
            ),
            (
                record("100644", "120000", "T", "link.py"),
                Change("T", "link.py", "link.py", Side.FILE, Side.LINK),
            ),
            (
                record("000000", "160000", "A", "vendor/lib"),
                Change("A", "vendor/lib", "vendor/lib", Side.ABSENT, Side.GITLINK),
            ),
            (
                record("000000", "000000", "U", "conflict.py"),
                Change("U", "conflict.py", "conflict.py", Side.ABSENT, Side.ABSENT),
            ),
        ],
        ids=[
            "modified",
            "added",
            "deleted",
            "renamed",
            "copied",
            "to-symlink",
            "gitlink",
            "unmerged",
        ],
    )
    def test_each_status_reads_as_its_change(self, raw, expected):
        assert gitchanges.parse_changes(raw) == (expected,)

    def test_several_records_keep_their_order(self):
        raw = record("100644", "100644", "R100", "a.py", "b.py") + record(
            "100644", "100644", "M", "c.py"
        )
        assert [change.path for change in gitchanges.parse_changes(raw)] == ["b.py", "c.py"]

    def test_no_output_is_no_change(self):
        assert gitchanges.parse_changes(b"") == ()

    def test_a_name_that_is_not_utf8_is_replaced_rather_than_dropped(self):
        raw = b":100644 100644 b1 00 M\0name\xff.py\0"
        (change,) = gitchanges.parse_changes(raw)
        assert change.path == "name\N{REPLACEMENT CHARACTER}.py"

    @pytest.mark.parametrize(
        ("raw", "reason"),
        [
            (b"a.py\0", "does not open with"),
            (b":100644 100644 b1 00\0a.py\0", "does not open with"),
            (b":100644 100644 b1 00 R100\0a.py\0", "ends before its path"),
            (b":100644 040000 b1 00 M\0a.py\0", "names no file type"),
        ],
        ids=["no-meta", "no-status", "short-rename", "tree-mode"],
    )
    def test_a_record_the_parser_cannot_read_is_refused(self, raw, reason):
        with pytest.raises(OperationFailed, match=reason):
            gitchanges.parse_changes(raw)


class TestParseUntracked:
    def test_files_are_additions_and_a_trailing_slash_is_a_nested_repository(self):
        assert gitchanges.parse_untracked(b"new.py\0nested/\0") == (
            Change(gitchanges.UNTRACKED, "new.py", "new.py", Side.ABSENT, Side.FILE),
            Change(gitchanges.UNTRACKED, "nested", "nested", Side.ABSENT, Side.GITLINK),
        )

    def test_no_output_is_no_file(self):
        assert gitchanges.parse_untracked(b"") == ()


class TestLooksBinary:
    def test_a_nul_byte_near_the_start_is_binary(self):
        assert gitchanges.looks_binary(b"bin\0ary")

    def test_a_nul_byte_past_the_probe_is_not(self):
        assert not gitchanges.looks_binary(b"x" * gitchanges.BINARY_PROBE_BYTES + b"\0")

    def test_text_is_not_binary(self):
        assert not gitchanges.looks_binary("def f():\n    return 'é'\n".encode())
