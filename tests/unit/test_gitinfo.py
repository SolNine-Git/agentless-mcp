"""Git state reading: real repositories in tmp_path, no ambient config."""

import errno
import os
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

    def test_a_nested_directory_says_whose_state_it_reported(self, make_git_repo):
        """Git answers for the enclosing repository, and the note says so.

        A directory inside a larger repository -- a vendored tree, a snapshot
        never given a git of its own -- is served that repository's HEAD and
        dirty count. A reader with only the receipt cannot tell, so the answer
        is qualified rather than quietly wrong.
        """
        root = make_git_repo(SAMPLE)
        snapshot = gitinfo.snapshot(root / "sub")

        assert "is not the top of its git repository" in snapshot.note
        assert str(root.resolve()) in snapshot.note

    def test_a_nested_directory_still_reports_the_head_it_is_cached_under(self, make_git_repo):
        """Only the note is added; the SHAs and count are what they were."""
        root = make_git_repo(SAMPLE)
        (root / "a.py").write_text("x = 2\n", encoding="utf-8")
        nested = gitinfo.snapshot(root / "sub")

        assert nested.head_sha == gitinfo.snapshot(root).head_sha
        assert nested.dirty_count == 1

    def test_the_repository_root_itself_carries_no_note(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        assert gitinfo.snapshot(root).note == ""

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

    def test_a_process_that_cannot_be_spawned_degrades_with_its_reason(
        self, monkeypatch, make_git_repo
    ):
        """The OSError arm: the host, not git, is what failed.

        ``FileNotFoundError`` is the one every reader thinks of, and it is a
        subclass. The arm that matters under load is its sibling -- EMFILE,
        ENOMEM, EAGAIN -- where git is installed and the process could not be
        started anyway. The whole point of this module is that a snapshot
        degrades to a note, so this must not reach the caller as a raise.
        """
        root = make_git_repo(SAMPLE)
        real_run = subprocess.run

        def cannot_spawn(command, **kwargs):
            # Root discovery still works, so the snapshot gets past it and
            # reaches the three reads whose notes are the thing under test.
            if "--show-toplevel" in command:
                return real_run(command, **kwargs)
            raise OSError(errno.EMFILE, os.strerror(errno.EMFILE))

        monkeypatch.setattr(subprocess, "run", cannot_spawn)
        snapshot = gitinfo.snapshot(root)

        assert snapshot.head_sha is None
        assert snapshot.tree_oid is None
        assert snapshot.dirty_count is None
        assert os.strerror(errno.EMFILE) in snapshot.note
        assert "could not be run" in snapshot.note

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


def _commit_all(root, message):
    """One more commit in a fixture repository, with the pinned test identity."""
    subprocess.run(
        [
            "git",
            "-C",
            str(root),
            "-c",
            "user.email=tests@example.invalid",
            "-c",
            "user.name=agentless-mcp tests",
            "commit",
            "-am",
            message,
        ],
        check=True,
        capture_output=True,
        timeout=30,
    )


class TestCommitChurn:
    """The windowed per-path commit facts the map header spends."""

    def test_counts_and_last_timestamp_per_requested_path(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        (root / "a.py").write_text("x = 2\n", encoding="utf-8")
        _commit_all(root, "touch a")

        facts = gitinfo.commit_churn(root, ["a.py", "sub/b.py"])

        assert facts is not None
        assert facts["a.py"].commits == 2
        assert facts["sub/b.py"].commits == 1
        assert facts["a.py"].last_commit_ts is not None
        assert facts["sub/b.py"].last_commit_ts is not None
        assert facts["a.py"].last_commit_ts >= facts["sub/b.py"].last_commit_ts

    def test_a_root_outside_git_answers_none_not_zero(self, tmp_path):
        """None is "git could not answer"; zeros would claim quiet history."""
        assert gitinfo.commit_churn(tmp_path, ["a.py"]) is None

    def test_a_path_with_no_commits_gets_zero_and_no_timestamp(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        facts = gitinfo.commit_churn(root, ["never_committed.py"])

        assert facts == {"never_committed.py": gitinfo.ChurnFact(commits=0, last_commit_ts=None)}

    def test_an_all_digit_filename_is_not_read_as_a_timestamp(self, make_git_repo):
        root = make_git_repo({"2024": "x = 1\n", "a.py": "y = 1\n"})
        facts = gitinfo.commit_churn(root, ["2024", "a.py"])

        assert facts is not None
        assert facts["2024"].commits == 1
        assert facts["a.py"].commits == 1

    def test_paths_are_relative_to_the_served_root_not_the_toplevel(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        facts = gitinfo.commit_churn(root / "sub", ["b.py"])

        assert facts is not None
        assert facts["b.py"].commits == 1

    def test_no_paths_asks_git_nothing(self, tmp_path):
        assert gitinfo.commit_churn(tmp_path, []) == {}
