"""Repository resolution and the allowlist refusal."""

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
        root = make_git_repo({"a.py": "x = 1\n"})
        ctx = resolve_repo(root, None)
        assert ctx.head_sha is not None
        assert ctx.dirty_count == 0


class TestAllowlist:
    def test_an_exact_match_is_accepted(self, tmp_path):
        allowed = tmp_path / "one"
        allowed.mkdir()
        ctx = resolve_repo(str(allowed), [allowed])
        assert ctx.root == allowed.resolve()

    def test_a_subdirectory_of_an_allowed_root_is_refused(self, tmp_path):
        """Containment is the wrong test: a repository is the unit of allowing.

        Accepting anything under an allowed root is how a workspace of seven
        repositories quietly becomes one.
        """
        allowed = tmp_path / "one"
        (allowed / "src").mkdir(parents=True)
        with pytest.raises(SecurityRefusal):
            resolve_repo(str(allowed / "src"), [allowed])

    def test_a_symlink_into_an_allowed_root_is_resolved_before_the_check(self, tmp_path):
        allowed = tmp_path / "one"
        allowed.mkdir()
        link = tmp_path / "link"
        link.symlink_to(allowed, target_is_directory=True)

        ctx = resolve_repo(str(link), [allowed])
        assert ctx.root == allowed.resolve()

    def test_a_symlink_out_of_the_allowlist_is_refused(self, tmp_path):
        allowed = tmp_path / "one"
        allowed.mkdir()
        outside = tmp_path / "elsewhere"
        outside.mkdir()
        link = allowed / "escape"
        link.symlink_to(outside, target_is_directory=True)

        with pytest.raises(SecurityRefusal):
            resolve_repo(str(link), [allowed])

    def test_the_refusal_lists_resolved_roots_and_never_the_raw_argument(self, tmp_path):
        allowed = tmp_path / "one"
        allowed.mkdir()
        other = tmp_path / "two"
        other.mkdir()
        secret = tmp_path / "not-allowed-secret-name"

        with pytest.raises(SecurityRefusal) as caught:
            resolve_repo(str(secret), [allowed, other])

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
