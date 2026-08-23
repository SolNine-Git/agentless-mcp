"""Security tests for path containment and walk bounds.

Each one asserts the specific message the refusal carries: a bound that
refuses with a vague message is a bound an operator cannot act on.
"""

import os

import pytest

from agentless_mcp.util.errors import (
    AtlasError,
    RepoResolutionError,
    SecurityRefusal,
    WalkBoundExceeded,
)
from agentless_mcp.util.fslimits import bounded_walk, contained_path, read_bounded


@pytest.fixture
def root(tmp_path):
    """A repository root with one file in it."""
    (tmp_path / "repo").mkdir()
    (tmp_path / "repo" / "app.py").write_text("x = 1\n", encoding="utf-8")
    return tmp_path / "repo"


class TestContainedPath:
    def test_relative_path_inside_root_resolves(self, root):
        assert contained_path(root, "app.py") == (root / "app.py").resolve()

    def test_absolute_path_inside_root_is_allowed(self, root):
        assert contained_path(root, str(root / "app.py")) == (root / "app.py").resolve()

    def test_missing_file_still_resolves(self, root):
        assert contained_path(root, "nope.py") == (root / "nope.py").resolve()

    def test_dotdot_escape_is_refused(self, root):
        with pytest.raises(SecurityRefusal) as caught:
            contained_path(root, "../outside.py")
        message = str(caught.value)
        assert message.startswith("path refused: resolved to ")
        assert "which is outside the root" in message
        assert str(root.resolve()) in message

    def test_absolute_escape_is_refused(self, root):
        with pytest.raises(SecurityRefusal, match="which is outside the root"):
            contained_path(root, "/etc/passwd")

    def test_a_path_the_filesystem_cannot_name_is_a_typed_refusal(self, root):
        # A NUL byte reaches this from JSON tool arguments; the refusal has to
        # stay inside the error hierarchy both adapters catch.
        with pytest.raises(SecurityRefusal, match="path refused: not a usable filesystem path"):
            contained_path(root, "a\0b")

    def test_symlink_escape_is_refused(self, root, tmp_path):
        secret = tmp_path / "secret.txt"
        secret.write_text("classified\n", encoding="utf-8")
        (root / "link.txt").symlink_to(secret)

        with pytest.raises(SecurityRefusal) as caught:
            contained_path(root, "link.txt")
        # The refusal names the resolved target, never the argument given.
        assert str(secret.resolve()) in str(caught.value)
        assert "link.txt" not in str(caught.value)


class TestBoundedWalk:
    def test_yields_files_inside_root(self, root):
        (root / "pkg").mkdir()
        (root / "pkg" / "mod.py").write_text("y = 2\n", encoding="utf-8")
        found = sorted(path.relative_to(root).as_posix() for path in bounded_walk(root))
        assert found == ["app.py", "pkg/mod.py"]

    def test_depth_bomb_is_refused(self, root):
        deep = root
        for index in range(25):
            deep = deep / f"level{index}"
        deep.mkdir(parents=True)
        (deep / "buried.py").write_text("z = 3\n", encoding="utf-8")

        with pytest.raises(WalkBoundExceeded) as caught:
            list(bounded_walk(root, max_depth=20))
        message = str(caught.value)
        assert "walk refused: directory depth 21 exceeds the limit of 20" in message
        assert "point the call at a subdirectory instead" in message

    def test_file_count_bomb_is_refused(self, root):
        for index in range(12):
            (root / f"file{index}.py").write_text("pass\n", encoding="utf-8")

        with pytest.raises(WalkBoundExceeded) as caught:
            list(bounded_walk(root, max_files=5))
        message = str(caught.value)
        assert "walk refused: more than 5 files under" in message
        assert "raise the file bound" in message

    def test_byte_bomb_is_refused(self, root):
        (root / "big.bin").write_text("x" * 4096, encoding="utf-8")

        with pytest.raises(WalkBoundExceeded, match="raise the byte bound"):
            list(bounded_walk(root, max_bytes=1024))

    def test_escaping_symlink_is_skipped_not_followed(self, root, tmp_path):
        outside = tmp_path / "outside"
        outside.mkdir()
        (outside / "secret.py").write_text("token = 1\n", encoding="utf-8")
        (root / "escape").symlink_to(outside, target_is_directory=True)
        (root / "escape.py").symlink_to(outside / "secret.py")

        found = sorted(path.relative_to(root).as_posix() for path in bounded_walk(root))
        assert found == ["app.py"]

    def test_non_directory_root_is_reported(self, root):
        with pytest.raises(RepoResolutionError, match="not a directory"):
            list(bounded_walk(root / "app.py"))

    def test_include_filter_selects_files(self, root):
        (root / "notes.txt").write_text("hello\n", encoding="utf-8")
        found = sorted(
            path.name for path in bounded_walk(root, include=lambda rel: rel.suffix == ".py")
        )
        assert found == ["app.py"]


class TestReadBounded:
    def test_reads_a_small_file(self, root):
        result = read_bounded(root / "app.py")
        assert result.text == "x = 1\n"
        assert result.skipped is None

    def test_oversize_file_is_skipped_with_a_reason(self, root):
        big = root / "big.py"
        big.write_text("#" * 5000, encoding="utf-8")

        result = read_bounded(big, max_bytes=1000)
        assert result.text is None
        assert result.skipped == "skipped: 5000 bytes exceeds the per-file cap of 1000 bytes"

    def test_a_stale_stat_cannot_bypass_the_read_cap(self, root, monkeypatch):
        # Named for what it asserts. `st_size` is made to under-report, and
        # the reported size comes from the bytes actually read rather than
        # from the stat -- so a file whose size the filesystem answers wrongly
        # is still capped.
        understated = root / "understated.py"
        understated.write_bytes(b"x" * 100)
        real_fstat = os.fstat

        def stale_size(descriptor):
            observed = real_fstat(descriptor)
            values = list(observed)
            values[6] = 4
            return os.stat_result(values)

        monkeypatch.setattr(os, "fstat", stale_size)

        result = read_bounded(understated, max_bytes=4)

        assert result.text is None
        assert result.skipped == "skipped: 5 bytes exceeds the per-file cap of 4 bytes"

    def test_a_negative_cap_is_refused_as_the_package_error(self, root):
        # The one bound in this module a caller could get wrong. It raised a
        # bare ValueError, which is not what the adapters catch on.
        readable = root / "small.py"
        readable.write_text("x", encoding="utf-8")

        with pytest.raises(AtlasError, match="max_bytes must be at least 0"):
            read_bounded(readable, max_bytes=-1)

    def test_missing_file_is_reported_not_raised(self, root):
        result = read_bounded(root / "gone.py")
        assert result.text is None
        assert result.skipped is not None
        assert result.skipped.startswith("unreadable: ")


class TestSymlinkLoops:
    """A loop resolves differently on the two supported interpreters.

    On the declared 3.10 floor ``Path.resolve()`` raises ``RuntimeError`` for
    a symlink loop, and the filter here caught only ``ValueError`` and
    ``OSError``, so the error escaped untyped past the boundary the adapters
    catch on. From 3.13 the same input resolves to a path that does not
    exist, and the strict re-resolve is skipped, so it was accepted. Two
    opposite failures on the two versions in the support matrix.

    What this pins is the property that holds on both: the answer is either a
    path inside the root or this module's own refusal, never a stdlib error.
    """

    def test_a_loop_is_contained_or_refused_but_never_untyped(self, tmp_path):
        (tmp_path / "loop_a").symlink_to(tmp_path / "loop_b")
        (tmp_path / "loop_b").symlink_to(tmp_path / "loop_a")

        try:
            resolved = contained_path(tmp_path, "loop_a")
        except SecurityRefusal:
            return
        assert tmp_path in resolved.parents

    def test_a_loop_pointing_out_of_the_root_is_refused(self, tmp_path):
        root = tmp_path / "repo"
        root.mkdir()
        outside = tmp_path / "outside"
        outside.mkdir()
        (outside / "loop_a").symlink_to(outside / "loop_b")
        (outside / "loop_b").symlink_to(outside / "loop_a")
        (root / "escape").symlink_to(outside / "loop_a")

        with pytest.raises(SecurityRefusal):
            contained_path(root, "escape")
