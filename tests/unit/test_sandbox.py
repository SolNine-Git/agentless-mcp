"""Worktree lifecycle: the checkout is untouched and the scratch is removed.

The whole security posture of the write side rests on these two facts, so both
are asserted directly rather than inferred: ``git status --porcelain`` and
``rev-parse HEAD`` are compared before and after, and the scratch directory is
checked to be gone -- including when the body of the context manager raised,
which is the case a ``finally`` exists for and the one nobody notices is
broken until a cache directory fills up.
"""

import subprocess

import pytest

from agentless_mcp.core import cache, sandbox
from agentless_mcp.util.errors import AtlasError, RepoResolutionError

FILES = {
    "app.py": "def add(a, b):\n    return a + b\n",
    "README.md": "# fixture\n",
}


@pytest.fixture
def repo(make_git_repo):
    """A committed one-file git repository."""
    return make_git_repo(FILES)


def git(root, *arguments):
    """Run one git command and return its stdout."""
    return subprocess.run(
        ["git", "-C", str(root), *arguments],
        check=True,
        capture_output=True,
        timeout=30,
    ).stdout.decode()


class TestWorktree:
    def test_it_yields_a_populated_checkout_at_head(self, repo):
        with sandbox.worktree(repo) as tree:
            assert (tree / "app.py").read_text(encoding="utf-8") == FILES["app.py"]
            assert tree != repo

    def test_the_scratch_lives_outside_the_repository(self, repo, isolated_cache_home):
        with sandbox.worktree(repo) as tree:
            assert repo not in tree.parents
            assert isolated_cache_home in tree.parents

    def test_the_checkout_is_bit_identical_afterwards(self, repo):
        before_status = git(repo, "status", "--porcelain")
        before_head = git(repo, "rev-parse", "HEAD")
        before_source = (repo / "app.py").read_text(encoding="utf-8")

        with sandbox.worktree(repo) as tree:
            (tree / "app.py").write_text("def add(a, b):\n    return a - b\n", encoding="utf-8")

        assert git(repo, "status", "--porcelain") == before_status
        assert git(repo, "rev-parse", "HEAD") == before_head
        assert (repo / "app.py").read_text(encoding="utf-8") == before_source

    def test_the_scratch_is_removed_on_success(self, repo):
        with sandbox.worktree(repo) as tree:
            captured = tree
        assert not captured.exists()

    def test_the_scratch_is_removed_when_the_body_raises(self, repo):
        captured = []

        def fail_inside():
            with sandbox.worktree(repo) as tree:
                captured.append(tree)
                message = "deliberate"
                raise RuntimeError(message)

        with pytest.raises(RuntimeError, match="deliberate"):
            fail_inside()

        assert captured
        assert not captured[0].exists()

    def test_the_repository_forgets_the_worktree_afterwards(self, repo):
        with sandbox.worktree(repo) as tree:
            recorded = str(tree)
        assert recorded not in git(repo, "worktree", "list")

    def test_a_subdirectory_root_yields_that_subdirectory(self, make_git_repo):
        """Git worktrees are whole-repository; the yielded path is not."""
        root = make_git_repo({"pkg/app.py": FILES["app.py"], "README.md": "# top\n"})
        with sandbox.worktree(root / "pkg") as tree:
            assert tree.name == "pkg"
            assert (tree / "app.py").read_text(encoding="utf-8") == FILES["app.py"]
        assert not tree.exists()

    def test_two_worktrees_do_not_collide(self, repo):
        with sandbox.worktree(repo) as first, sandbox.worktree(repo) as second:
            assert first != second

    def test_a_directory_outside_git_is_refused(self, tmp_path):
        plain = tmp_path / "plain"
        plain.mkdir()

        def enter():
            with sandbox.worktree(plain):
                pass

        with pytest.raises(RepoResolutionError, match="not inside a git repository"):
            enter()

    def test_the_scratch_root_is_under_the_cache_home(self, isolated_cache_home):
        assert sandbox.scratch_root().parent == cache.cache_root()
        assert isolated_cache_home in sandbox.scratch_root().parents


class TestDiff:
    def test_a_written_change_shows_up_as_a_unified_diff(self, repo):
        with sandbox.worktree(repo) as tree:
            (tree / "app.py").write_text("def add(a, b):\n    return a - b\n", encoding="utf-8")
            text = sandbox.diff(tree)

        assert "--- a/app.py" in text
        assert "+++ b/app.py" in text
        assert "-    return a + b" in text
        assert "+    return a - b" in text

    def test_an_untouched_worktree_diffs_to_nothing(self, repo):
        with sandbox.worktree(repo) as tree:
            assert sandbox.diff(tree) == ""


class TestRunGit:
    def test_a_failing_command_raises_with_the_reason(self, repo):
        with pytest.raises(AtlasError, match="exited"):
            sandbox.run_git(repo, ["rev-parse", "refs/heads/no-such-branch"])
