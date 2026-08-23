"""Git state reading: real repositories in tmp_path, no ambient config."""

import subprocess

import pytest

from agentless_mcp.core import gitinfo, sandbox, treewalk

SAMPLE = {"a.py": "x = 1\n", "sub/b.py": "y = 2\n"}


class TestGitRoot:
    def test_finds_the_top_level_from_a_subdirectory(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        assert gitinfo.git_root(root / "sub") == root.resolve()

    def test_finds_the_top_level_from_a_file(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        assert gitinfo.git_root(root / "a.py") == root.resolve()

    def test_returns_none_outside_a_repository(self, tmp_path):
        plain = tmp_path / "plain"
        plain.mkdir()
        assert gitinfo.git_root(plain) is None


class TestSnapshot:
    def test_reports_head_tree_and_a_clean_tree(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        snapshot = gitinfo.snapshot(root)

        assert snapshot.head_sha is not None
        assert len(snapshot.head_sha) == gitinfo.SHORT_SHA_LENGTH
        assert snapshot.tree_oid is not None
        assert snapshot.dirty_count == 0
        assert snapshot.note == ""

    def test_counts_modified_and_untracked_paths(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        (root / "a.py").write_text("x = 2\n", encoding="utf-8")
        (root / "new.py").write_text("z = 3\n", encoding="utf-8")

        assert gitinfo.dirty_count(root) == 2

    def test_non_git_directory_is_all_unknown_with_a_note(self, tmp_path):
        plain = tmp_path / "plain"
        plain.mkdir()
        snapshot = gitinfo.snapshot(plain)

        assert (snapshot.head_sha, snapshot.tree_oid, snapshot.dirty_count) == (None, None, None)
        assert "not inside a git repository" in snapshot.note

    def test_a_repository_without_commits_reports_the_reason(self, tmp_path):
        root = tmp_path / "empty"
        root.mkdir()
        subprocess.run(
            ["git", "-C", str(root), "init", "-b", "main"],
            check=True,
            capture_output=True,
            timeout=30,
        )
        snapshot = gitinfo.snapshot(root)

        assert snapshot.head_sha is None
        assert snapshot.dirty_count == 0
        assert "rev-parse" in snapshot.note


class TestDegradation:
    def test_a_missing_git_binary_is_a_note_not_an_exception(self, monkeypatch, tmp_path):
        message = "git"

        def missing(*args, **kwargs):
            raise FileNotFoundError(message)

        monkeypatch.setattr(subprocess, "run", missing)
        assert gitinfo.git_root(tmp_path) is None
        assert gitinfo.head_sha(tmp_path) is None
        assert gitinfo.dirty_count(tmp_path) is None

    def test_a_timeout_leaves_the_dirty_count_unknown(self, monkeypatch, make_git_repo):
        root = make_git_repo(SAMPLE)

        def slow(command, **kwargs):
            raise subprocess.TimeoutExpired(command, gitinfo.GIT_TIMEOUT_SECONDS)

        monkeypatch.setattr(subprocess, "run", slow)
        assert gitinfo.dirty_count(root) is None

    def test_every_invocation_is_bounded_and_declines_optional_locks(
        self, monkeypatch, make_git_repo
    ):
        """Both properties are asserted on every argv the module produces.

        A timeout that is right on three of four calls is not a bound, and a
        lock taken on one call is enough to write into a repository this tool
        promises never to write to.
        """
        root = make_git_repo(SAMPLE)
        calls = []
        real = subprocess.run

        def record(command, **kwargs):
            calls.append((command, kwargs.get("timeout")))
            return real(command, **kwargs)

        monkeypatch.setattr(subprocess, "run", record)
        gitinfo.snapshot(root)

        assert len(calls) >= 4, "expected rev-parse x3 plus status"
        for command, timeout in calls:
            assert command[: 1 + len(gitinfo.HARDENING_PREFIX)] == [
                "git",
                *gitinfo.HARDENING_PREFIX,
            ]
            assert timeout == gitinfo.GIT_TIMEOUT_SECONDS

    def test_every_package_git_argv_has_the_same_hardening_prefix(self, monkeypatch, tmp_path):
        calls = []

        def record(command, **_kwargs):
            calls.append(command)
            return subprocess.CompletedProcess(command, 0, stdout=b"", stderr=b"")

        monkeypatch.setattr(subprocess, "run", record)

        gitinfo.head_sha(tmp_path)
        treewalk._git_listed_paths(tmp_path)
        sandbox.run_git(tmp_path, ["status", "--porcelain"])

        expected = ["git", *gitinfo.HARDENING_PREFIX]
        assert len(calls) == 3
        assert all(command[: len(expected)] == expected for command in calls)


class TestAmbientGitEnvironmentCannotRedirect:
    """The ``-C`` this package passes has to be what decides the repository.

    Reproduced before the fix: with ``GIT_DIR`` pointing at an unrelated
    repository, the receipt for the analysed repository carried the *other*
    repository's HEAD; with ``GIT_INDEX_FILE`` naming a path that does not
    exist, a clean tree reported dirty files. The receipt is how an agent
    knows which commit an answer describes.
    """

    @pytest.fixture
    def two_repos(self, make_git_repo):
        analysed = make_git_repo({"m.py": "value = 1\n"}, name="analysed")
        elsewhere = make_git_repo({"n.py": "value = 2\n"}, name="elsewhere")
        return analysed, elsewhere

    def test_a_git_dir_pointing_elsewhere_does_not_move_the_head(self, two_repos, monkeypatch):
        analysed, elsewhere = two_repos
        expected = gitinfo.snapshot(analysed).head_sha

        monkeypatch.setenv("GIT_DIR", str(elsewhere / ".git"))

        assert gitinfo.snapshot(analysed).head_sha == expected

    def test_a_broken_index_file_does_not_make_a_clean_tree_dirty(
        self, two_repos, monkeypatch, tmp_path
    ):
        analysed, _ = two_repos
        assert gitinfo.snapshot(analysed).dirty_count == 0

        monkeypatch.setenv("GIT_INDEX_FILE", str(tmp_path / "nonexistent"))

        assert gitinfo.snapshot(analysed).dirty_count == 0

    def test_the_whole_git_family_is_stripped_and_nothing_else_is(self, monkeypatch):
        # A denylist of the variables known to hurt today would be a list git
        # is free to extend. The whole prefix is stripped instead, and the
        # configuration this package needs travels on the argv.
        monkeypatch.setenv("GIT_DIR", "/somewhere")
        monkeypatch.setenv("GIT_SOMETHING_INVENTED_LATER", "1")
        monkeypatch.setenv("PATH", "/usr/bin")
        monkeypatch.setenv("HOME", "/home/someone")

        environment = gitinfo.subprocess_env()

        assert not [name for name in environment if name.startswith("GIT_")]
        assert environment["PATH"] == "/usr/bin"
        assert environment["HOME"] == "/home/someone"
