"""Tests for the gitignore-aware walker and the tree renderer.

Git identity is pinned per invocation and every repository is built under
tmp_path, so nothing here depends on the developer's git config or on a
repository that happens to exist on the machine.
"""

import subprocess

import pytest

from agentless_mcp.core.treewalk import RepoFile, render_tree, walk_repo
from agentless_mcp.util.errors import WalkBoundExceeded

GIT_IDENTITY = ["-c", "user.name=test", "-c", "user.email=test@test"]


def git(repo, *args):
    """Run git in ``repo`` with a pinned identity and no ambient config."""
    completed = subprocess.run(
        ["git", *GIT_IDENTITY, "-C", str(repo), *args],
        capture_output=True,
        check=True,
        timeout=30,
        env={"HOME": str(repo.parent), "PATH": "/usr/bin:/bin", "GIT_CONFIG_NOSYSTEM": "1"},
    )
    return completed.stdout.decode()


@pytest.fixture
def git_repo(tmp_path):
    """A git repository with one ignored file and one nested package."""
    repo = tmp_path / "repo"
    (repo / "pkg").mkdir(parents=True)
    (repo / ".gitignore").write_text("build/\n*.log\n", encoding="utf-8")
    (repo / "app.py").write_text("x = 1\n", encoding="utf-8")
    (repo / "pkg" / "mod.py").write_text("y = 2\n", encoding="utf-8")
    (repo / "debug.log").write_text("noise\n", encoding="utf-8")
    (repo / "build").mkdir()
    (repo / "build" / "artifact.bin").write_text("binary\n", encoding="utf-8")

    git(repo, "init", "--quiet")
    git(repo, "add", ".")
    git(repo, "commit", "--quiet", "-m", "initial")
    return repo


class TestWalkRepo:
    def test_gitignored_paths_are_excluded(self, git_repo):
        paths = [f.path for f in walk_repo(git_repo)]
        assert paths == [".gitignore", "app.py", "pkg/mod.py"]

    def test_untracked_but_unignored_file_is_included(self, git_repo):
        (git_repo / "new.py").write_text("z = 3\n", encoding="utf-8")
        paths = [f.path for f in walk_repo(git_repo)]
        assert "new.py" in paths
        assert "debug.log" not in paths

    def test_non_git_directory_falls_back_to_the_bounded_walk(self, tmp_path):
        plain = tmp_path / "plain"
        (plain / "sub").mkdir(parents=True)
        (plain / "a.py").write_text("a = 1\n", encoding="utf-8")
        (plain / "sub" / "b.py").write_text("b = 2\n", encoding="utf-8")
        # Not a git repository, so .gitignore is just another file.
        (plain / ".gitignore").write_text("*.py\n", encoding="utf-8")

        paths = [f.path for f in walk_repo(plain)]
        assert paths == [".gitignore", "a.py", "sub/b.py"]

    def test_sizes_are_reported(self, git_repo):
        files = {f.path: f.size for f in walk_repo(git_repo)}
        assert files["app.py"] == len("x = 1\n")

    def test_file_bound_is_enforced_inside_a_repository(self, git_repo):
        with pytest.raises(WalkBoundExceeded, match="raise the file bound"):
            walk_repo(git_repo, max_files=2)

    def test_a_tracked_symlink_escaping_the_root_is_not_listed(self, git_repo, tmp_path):
        secret = tmp_path / "secret.txt"
        secret.write_text("classified\n", encoding="utf-8")
        (git_repo / "leak.py").symlink_to(secret)
        git(git_repo, "add", "leak.py")
        git(git_repo, "commit", "--quiet", "-m", "leak")

        paths = [f.path for f in walk_repo(git_repo)]
        assert "leak.py" not in paths

    def test_a_subdirectory_root_still_honours_the_repository_gitignore(self, git_repo):
        # `build/` and `*.log` are ignored by the root .gitignore, which the
        # non-git fallback walk knows nothing about.
        (git_repo / "pkg" / "trace.log").write_text("noise\n", encoding="utf-8")

        paths = [f.path for f in walk_repo(git_repo / "pkg")]
        assert paths == ["mod.py"]


class TestRenderTree:
    def test_directories_carry_a_trailing_slash(self):
        files = [RepoFile("app.py", 6), RepoFile("pkg/mod.py", 6)]
        assert render_tree(files) == "app.py\npkg/\n    mod.py\n"

    def test_depth_limit_marks_what_it_hides(self):
        files = [RepoFile("a/b/c/d/e.py", 1)]
        rendered = render_tree(files, depth=2)
        assert rendered == "a/\n    b/\n        ...\n"

    def test_max_entries_truncation_is_marked(self):
        files = [RepoFile(f"f{index}.py", 1) for index in range(10)]
        rendered = render_tree(files, max_entries=3)
        lines = rendered.splitlines()
        assert lines[:3] == ["f0.py", "f1.py", "f2.py"]
        assert lines[-1] == "... 7 more entries truncated (max_entries=3)"

    def test_empty_repository_renders_empty(self):
        assert render_tree([]) == ""
