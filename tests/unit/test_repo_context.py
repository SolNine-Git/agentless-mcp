"""Repository resolution, the allowlist refusal and the cache receipt."""

import sqlite3
from dataclasses import replace

import pytest

from agentless_mcp.application.repo_context import resolve_repo, resolved_allowlist
from agentless_mcp.util.errors import RepoResolutionError, SecurityRefusal


class TestCliMode:
    def test_allowlist_none_accepts_any_directory(self, tmp_path):
        ctx = resolve_repo(tmp_path, None)
        assert ctx.root == tmp_path.resolve()

    def test_a_file_is_not_a_repository(self, tmp_path):
        target = tmp_path / "a.py"
        target.write_text("x = 1\n", encoding="utf-8")
        with pytest.raises(RepoResolutionError, match="not a directory"):
            resolve_repo(target, None)

    def test_git_state_is_snapshotted_once(self, make_git_repo):
        """The context reports the state the answer was computed from.

        Dirtying the tree after the call must not move the receipt: a context
        that re-read git per render would describe a repository that is not
        the one the answer came from, which is why the dataclass is frozen.
        """
        root = make_git_repo({"a.py": "x = 1\n"})
        ctx = resolve_repo(root, None)
        assert ctx.head_sha is not None
        assert ctx.dirty_count == 0

        (root / "a.py").write_text("x = 2\n", encoding="utf-8")
        (root / "b.py").write_text("y = 3\n", encoding="utf-8")

        assert ctx.dirty_count == 0
        assert resolve_repo(root, None).dirty_count == 2


class TestAllowlist:
    def test_an_exact_match_is_accepted(self, tmp_path):
        allowed = tmp_path / "one"
        allowed.mkdir()
        ctx = resolve_repo(str(allowed), resolved_allowlist([allowed]))
        assert ctx.root == allowed.resolve()

    def test_a_subdirectory_of_an_allowed_root_is_refused(self, tmp_path):
        """Containment is the wrong test: a repository is the unit of allowing.

        Accepting anything under an allowed root is how a workspace of seven
        repositories quietly becomes one.
        """
        allowed = tmp_path / "one"
        (allowed / "src").mkdir(parents=True)
        with pytest.raises(SecurityRefusal):
            resolve_repo(str(allowed / "src"), resolved_allowlist([allowed]))

    def test_a_symlink_into_an_allowed_root_is_resolved_before_the_check(self, tmp_path):
        allowed = tmp_path / "one"
        allowed.mkdir()
        link = tmp_path / "link"
        link.symlink_to(allowed, target_is_directory=True)

        ctx = resolve_repo(str(link), resolved_allowlist([allowed]))
        assert ctx.root == allowed.resolve()

    def test_a_symlink_out_of_the_allowlist_is_refused(self, tmp_path):
        allowed = tmp_path / "one"
        allowed.mkdir()
        outside = tmp_path / "elsewhere"
        outside.mkdir()
        link = allowed / "escape"
        link.symlink_to(outside, target_is_directory=True)

        with pytest.raises(SecurityRefusal):
            resolve_repo(str(link), resolved_allowlist([allowed]))

    def test_the_refusal_lists_resolved_roots_and_never_the_raw_argument(self, tmp_path):
        allowed = tmp_path / "one"
        allowed.mkdir()
        other = tmp_path / "two"
        other.mkdir()
        secret = tmp_path / "not-allowed-secret-name"

        with pytest.raises(SecurityRefusal) as caught:
            resolve_repo(str(secret), resolved_allowlist([allowed, other]))

        message = str(caught.value)
        assert str(allowed.resolve()) in message
        assert str(other.resolve()) in message
        assert "not-allowed-secret-name" not in message

    def test_an_empty_allowlist_refuses_everything_and_says_why(self, tmp_path):
        with pytest.raises(SecurityRefusal, match="started with no roots"):
            resolve_repo(str(tmp_path), [])

    def test_the_allowlist_is_realpathed_once_at_startup(self, tmp_path):
        real = tmp_path / "one"
        real.mkdir()
        link = tmp_path / "link"
        link.symlink_to(real, target_is_directory=True)

        assert resolved_allowlist([str(link)]) == [real.resolve()]

    def test_an_unresolved_entry_is_not_resolved_here(self, tmp_path):
        """One owner for the resolution, and it is ``resolved_allowlist``.

        An entry that never went through it refuses a root it would have
        admitted. That is the direction a mistake in this rule has to fail,
        and it is what stops a symlink retargeted after startup from quietly
        changing what the server authorises.
        """
        real = tmp_path / "one"
        real.mkdir()
        link = tmp_path / "link"
        link.symlink_to(real, target_is_directory=True)

        with pytest.raises(SecurityRefusal):
            resolve_repo(str(real), [link])


class _StubSource:
    """A symbol source that counts how often its receipt is read."""

    def __init__(self, receipt="g:1a2b3c4d fresh", error=None):
        self._receipt = receipt
        self._error = error
        self.reads = 0

    @property
    def receipt(self):
        self.reads += 1
        if self._error is not None:
            raise self._error
        return self._receipt

    def close(self):
        """Release nothing."""


class TestCacheReceipt:
    """The ``cache:`` half of the receipt, snapshotted like the git half."""

    def test_the_source_is_described_once_per_context(self, tmp_path):
        source = _StubSource()
        ctx = replace(resolve_repo(tmp_path, None), symbols=source)

        assert ctx.cache_receipt == "g:1a2b3c4d fresh"
        assert ctx.cache_receipt == "g:1a2b3c4d fresh"
        assert source.reads == 1

    def test_no_source_reads_none(self, tmp_path):
        assert resolve_repo(tmp_path, None).cache_receipt == "none"

    def test_a_failing_source_degrades_into_a_note(self, tmp_path):
        """A courtesy field must not take the answer with it.

        The git half of this receipt turns every failure into a note; the
        cache half reaches SQLite, which can raise on a locked or truncated
        index the answer never depended on.
        """
        source = _StubSource(error=sqlite3.DatabaseError("database disk image is malformed"))
        ctx = replace(resolve_repo(tmp_path, None), symbols=source)

        assert ctx.cache_receipt.startswith("none (cache status unavailable:")
        assert "malformed" in ctx.cache_receipt
