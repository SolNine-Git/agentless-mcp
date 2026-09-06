"""Git state reading: real repositories in tmp_path, no ambient config."""

import errno
import os
import subprocess
import sys

import pytest

from agentless_mcp.core import gitinfo, sandbox, treewalk

SAMPLE = {"a.py": "x = 1\n", "sub/b.py": "y = 2\n"}


def git_out(root, *arguments):
    """Run one git command in ``root`` and return its stdout bytes."""
    return subprocess.run(
        ["git", "-C", str(root), *arguments], check=True, capture_output=True, timeout=30
    ).stdout


def sign_head_commit(root):
    """Rewrite HEAD's commit object with a gpgsig header, so git offers to verify it."""
    # A real signature is not needed and gpg is not assumed to exist: git
    # consults gpg.program because the header is present, not because it verifies.
    lines = []
    for line in git_out(root, "cat-file", "commit", "HEAD").decode().split("\n"):
        lines.append(line)
        if line.startswith("committer "):
            lines += [
                "gpgsig -----BEGIN PGP SIGNATURE-----",
                " ",
                " bm90IGEgc2lnbmF0dXJl",
                " -----END PGP SIGNATURE-----",
            ]
    written = (
        subprocess.run(
            ["git", "-C", str(root), "hash-object", "-t", "commit", "-w", "--stdin"],
            input="\n".join(lines).encode(),
            check=True,
            capture_output=True,
            timeout=30,
        )
        .stdout.decode()
        .strip()
    )
    git_out(root, "update-ref", "HEAD", written)


@pytest.fixture
def signature_bait(make_git_repo, tmp_path):
    """Build a repository whose HEAD is signed and whose gpg.program touches a marker."""

    def build(*, local_config=True):
        root = make_git_repo(SAMPLE)
        marker = tmp_path / "gpg-fired.txt"
        program = root / "gpg.sh"
        program.write_text(
            f'#!/bin/sh\necho fired > "{marker}"\ncat > /dev/null\n', encoding="utf-8"
        )
        program.chmod(0o755)
        sign_head_commit(root)
        if local_config:
            git_out(root, "config", "log.showSignature", "true")
            git_out(root, "config", "gpg.program", str(program))
        return root, marker, program

    return build


class SilentChild:
    """A spawned process that holds both pipes open and never writes or exits."""

    def __init__(self, command, **kwargs):
        self._writers = []
        self.stdout = self._reader()
        self.stderr = self._reader()
        self.returncode = None

    def _reader(self):
        read_fd, write_fd = os.pipe()
        self._writers.append(write_fd)
        return os.fdopen(read_fd, "rb")

    def kill(self):
        for fd in self._writers:
            os.close(fd)
        self._writers = []
        self.returncode = -9

    def wait(self, timeout=None):
        return self.returncode

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        if self._writers:
            self.kill()
        for pipe in (self.stdout, self.stderr):
            if pipe is not None:
                pipe.close()


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

        monkeypatch.setattr(subprocess, "Popen", missing)
        assert gitinfo.git_root(tmp_path) is None
        assert gitinfo.head_sha(tmp_path) is None
        assert gitinfo.dirty_count(tmp_path) is None

    def test_a_timeout_leaves_the_dirty_count_unknown(self, monkeypatch, make_git_repo):
        root = make_git_repo(SAMPLE)
        monkeypatch.setattr(gitinfo, "GIT_TIMEOUT_SECONDS", 0.05)
        monkeypatch.setattr(subprocess, "Popen", SilentChild)
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
        real_popen = subprocess.Popen

        def cannot_spawn(command, **kwargs):
            # Root discovery still works, so the snapshot gets past it and
            # reaches the three reads whose notes are the thing under test.
            if "--show-toplevel" in command:
                return real_popen(command, **kwargs)
            raise OSError(errno.EMFILE, os.strerror(errno.EMFILE))

        monkeypatch.setattr(subprocess, "Popen", cannot_spawn)
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
        argvs = []
        bounds = []
        real_popen = subprocess.Popen
        real_bounded = gitinfo.run_bounded

        def record_spawn(command, **kwargs):
            argvs.append(command)
            return real_popen(command, **kwargs)

        def record_bound(cwd, arguments, *, timeout, max_output_bytes):
            bounds.append((timeout, max_output_bytes))
            return real_bounded(cwd, arguments, timeout=timeout, max_output_bytes=max_output_bytes)

        monkeypatch.setattr(subprocess, "Popen", record_spawn)
        monkeypatch.setattr(gitinfo, "run_bounded", record_bound)
        gitinfo.snapshot(root)

        assert len(argvs) >= 4, "expected rev-parse x3 plus status"
        for command in argvs:
            assert command[: 1 + len(gitinfo.HARDENING_PREFIX)] == [
                "git",
                *gitinfo.HARDENING_PREFIX,
            ]
        assert len(bounds) == len(argvs)
        assert set(bounds) == {(gitinfo.GIT_TIMEOUT_SECONDS, gitinfo.MAX_RECEIPT_OUTPUT_BYTES)}

    def test_every_package_git_argv_has_the_same_hardening_prefix(self, monkeypatch, tmp_path):
        calls = []
        real_popen = subprocess.Popen

        def record_run(command, **_kwargs):
            calls.append(command)
            return subprocess.CompletedProcess(command, 0, stdout=b"", stderr=b"")

        def record_spawn(command, **kwargs):
            calls.append(command)
            return real_popen(command, **kwargs)

        # Two seams because the receipt reader spawns and the other two callers
        # still run to completion; the prefix has to be on the argv of each.
        monkeypatch.setattr(subprocess, "run", record_run)
        monkeypatch.setattr(subprocess, "Popen", record_spawn)

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

    def test_the_whole_git_family_is_stripped_except_the_two_config_names(self, monkeypatch):
        # A denylist of the variables known to hurt today would be a list git
        # is free to extend. The whole prefix is stripped instead, and the
        # configuration this package needs travels on the argv.
        monkeypatch.setenv("GIT_DIR", "/somewhere")
        monkeypatch.setenv("GIT_SOMETHING_INVENTED_LATER", "1")
        monkeypatch.setenv("GIT_CONFIG_GLOBAL", "/some/config")
        monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
        monkeypatch.setenv("PATH", "/usr/bin")
        monkeypatch.setenv("HOME", "/home/someone")

        environment = gitinfo.subprocess_env()

        assert [name for name in environment if name.startswith("GIT_")] == [
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_NOSYSTEM",
        ]
        assert environment["PATH"] == "/usr/bin"
        assert environment["HOME"] == "/home/someone"

    def test_the_kept_names_cannot_redirect_git_at_another_repository(self, two_repos, monkeypatch):
        """The two kept names say where config is read, never which repository is.

        That distinction is the whole reason they are exempt, so it is asserted
        rather than left to the reader of git-config(1).
        """
        analysed, elsewhere = two_repos
        expected = gitinfo.snapshot(analysed).head_sha

        monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(elsewhere / ".git" / "config"))
        monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")

        assert gitinfo.snapshot(analysed).head_sha == expected


class TestCommitChurn:
    """The windowed per-path commit facts the map header spends."""

    def test_counts_and_last_timestamp_per_requested_path(self, make_git_repo, commit_all):
        root = make_git_repo(SAMPLE)
        (root / "a.py").write_text("x = 2\n", encoding="utf-8")
        commit_all(root, "touch a")

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

    def test_a_non_ascii_filename_is_counted_not_reported_quiet(self, make_git_repo):
        """Git's default quoting octal-escapes non-ASCII paths in --name-only.

        Without ``core.quotePath=false`` on the argv, git prints the name as
        a quoted, octal-escaped C string, the exact match misses, and a file
        with real commits reads back as ``commits=0`` -- rendered downstream
        as the measured-quiet claim this feature promises cannot be faked.
        """
        root = make_git_repo({"café.py": "x = 1\n", "plain.py": "y = 1\n"})
        facts = gitinfo.commit_churn(root, ["café.py", "plain.py"])

        assert facts is not None
        assert facts["café.py"].commits == 1
        assert facts["café.py"].last_commit_ts is not None

    def test_a_dash_prefixed_filename_is_a_path_not_an_option(self, make_git_repo):
        """The ``--`` before the batch is what keeps this a pathspec."""
        root = make_git_repo({"-o.py": "x = 1\n"})
        facts = gitinfo.commit_churn(root, ["-o.py"])

        assert facts is not None
        assert facts["-o.py"].commits == 1

    def test_a_pathspec_magic_prefixed_filename_stays_literal(self, make_git_repo):
        """``--`` does not disable pathspec magic; the ``./`` prefix does.

        Without it a tracked file named ``:!x.py`` is parsed as an exclude
        pattern: it never matches itself, and it suppresses matches for the
        rest of the batch in the same call.
        """
        root = make_git_repo({":!x.py": "x = 1\n", "a.py": "y = 1\n"})
        facts = gitinfo.commit_churn(root, [":!x.py", "a.py"])

        assert facts is not None
        assert facts[":!x.py"].commits == 1
        assert facts["a.py"].commits == 1

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


class TestRunBounded:
    def test_a_successful_call_keeps_stdout_whole(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        outcome = gitinfo.run_bounded(
            root, ["rev-parse", "HEAD"], timeout=5.0, max_output_bytes=4096
        )

        assert outcome.returncode == 0
        assert outcome.truncated is False
        assert outcome.text is not None
        assert outcome.text.endswith("\n")
        assert len(outcome.text.strip()) == 40

    def test_the_argv_carries_the_hardening_prefix_and_a_c_locale(self, monkeypatch, tmp_path):
        seen = {}
        message = "git"

        def record(command, **kwargs):
            seen["command"] = command
            seen["env"] = kwargs["env"]
            raise FileNotFoundError(message)

        monkeypatch.setattr(subprocess, "Popen", record)
        outcome = gitinfo.run_bounded(tmp_path, ["log"], timeout=1.0, max_output_bytes=10)

        assert outcome.text is None
        assert "not installed" in outcome.note
        expected = ["git", *gitinfo.HARDENING_PREFIX]
        assert seen["command"][: len(expected)] == expected
        assert seen["env"]["LC_ALL"] == "C"
        assert not [
            name
            for name in seen["env"]
            if name.startswith("GIT_") and name not in gitinfo.GIT_CONFIG_KEPT
        ]

    def test_a_nonzero_exit_is_a_note_with_the_first_stderr_line(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        outcome = gitinfo.run_bounded(
            root, ["rev-parse", "--verify", "no-such-ref-zzz"], timeout=5.0, max_output_bytes=4096
        )

        assert outcome.text is None
        assert outcome.returncode not in (0, None)
        assert outcome.note.startswith(f"git rev-parse exited {outcome.returncode}: fatal:")

    def test_the_output_cap_cuts_stdout_and_flags_it(self, make_git_repo, commit_all):
        root = make_git_repo(SAMPLE)
        (root / "a.py").write_text("x = 2\n", encoding="utf-8")
        subprocess.run(
            ["git", "-C", str(root), "add", "-A"], check=True, capture_output=True, timeout=30
        )
        commit_all(root, "big\n\n" + "y" * 100_000)

        outcome = gitinfo.run_bounded(
            root, ["log", "-1", "--format=%B"], timeout=30.0, max_output_bytes=4096
        )

        assert outcome.truncated is True
        assert outcome.text is not None
        assert len(outcome.text.encode()) == 4096

    def test_a_deadline_kills_a_silent_child(self, monkeypatch, tmp_path):
        monkeypatch.setattr(subprocess, "Popen", SilentChild)
        outcome = gitinfo.run_bounded(tmp_path, ["log"], timeout=0.05, max_output_bytes=10)

        assert outcome.text is None
        assert outcome.note == "git log timed out after 0.05s"

    def test_a_process_that_cannot_be_spawned_degrades_with_its_reason(self, monkeypatch, tmp_path):
        def cannot_spawn(command, **kwargs):
            raise OSError(errno.EMFILE, "Too many open files")

        monkeypatch.setattr(subprocess, "Popen", cannot_spawn)
        outcome = gitinfo.run_bounded(tmp_path, ["log"], timeout=1.0, max_output_bytes=10)

        assert outcome.text is None
        assert outcome.note == "git log could not be run: Too many open files"


class TestRepositoryConfigCannotChooseAProgram:
    """``log.showSignature`` plus ``gpg.program`` turns a repository read into an exec.

    Both keys are repository-local, so a repository that ships them decides
    what runs the moment this package reads its history. The prefix pins the
    first key off, which is what makes the second unreachable.
    """

    def test_the_bait_fires_when_the_prefix_is_absent(self, signature_bait):
        """The control half: without it a green assertion below proves nothing."""
        root, marker, _ = signature_bait()
        git_out(root, "log", "--oneline")

        assert marker.exists(), "the fixture never fired; the assertions below are vacuous"

    def test_the_churn_log_runs_nothing(self, signature_bait):
        root, marker, _ = signature_bait()

        assert gitinfo.commit_churn(root, ["a.py"]) is not None
        assert not marker.exists(), "the repository's gpg.program ran under _run"

    def test_the_history_log_runs_nothing(self, signature_bait):
        root, marker, _ = signature_bait()
        outcome = gitinfo.run_bounded(
            root,
            ["log", "-L1,1:a.py", "--no-patch", "-z", "--format=%H", "--max-count=5", "--"],
            timeout=30.0,
            max_output_bytes=100_000,
        )

        assert outcome.note == ""
        assert not marker.exists(), "the repository's gpg.program ran under run_bounded"

    def test_the_snapshot_reads_run_nothing(self, signature_bait):
        root, marker, _ = signature_bait()

        assert gitinfo.snapshot(root).head_sha is not None
        assert not marker.exists()


class TestPinnedGlobalConfigIsHonoured:
    """The suite pins git's global config; the package's own calls must see that pin."""

    def test_the_pinned_global_config_reaches_the_package_s_git(
        self, make_git_repo, tmp_path, monkeypatch
    ):
        """``core.abbrev`` is observable in the output, so arrival is proved, not assumed."""
        root = make_git_repo(SAMPLE)
        config = tmp_path / "global.cfg"
        config.write_text("[core]\n\tabbrev = 12\n", encoding="utf-8")
        monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(config))

        outcome = gitinfo.run_bounded(
            root, ["rev-parse", "--short", "HEAD"], timeout=5.0, max_output_bytes=4096
        )

        assert outcome.text is not None
        assert len(outcome.text.strip()) == 12

    def test_a_global_show_signature_reaches_git_and_still_runs_nothing(
        self, signature_bait, tmp_path, monkeypatch
    ):
        root, marker, program = signature_bait(local_config=False)
        config = tmp_path / "global.cfg"
        config.write_text(
            f"[log]\n\tshowSignature = true\n[gpg]\n\tprogram = {program}\n", encoding="utf-8"
        )
        monkeypatch.setenv("GIT_CONFIG_GLOBAL", str(config))
        git_out(root, "log", "--oneline")
        assert marker.exists(), "the global config never reached git; the assertion is vacuous"
        marker.unlink()

        assert gitinfo.commit_churn(root, ["a.py"]) is not None
        assert not marker.exists()


class TestReceiptOutputCap:
    def test_output_past_the_cap_is_unknown_rather_than_partial(self, monkeypatch, make_git_repo):
        """Half a porcelain status undercounts a dirty tree, so it is not reported at all."""
        root = make_git_repo(SAMPLE)
        (root / "a.py").write_text("x = 2\n", encoding="utf-8")
        monkeypatch.setattr(gitinfo, "MAX_RECEIPT_OUTPUT_BYTES", 4)

        outcome = gitinfo._run(root, ["status", "--porcelain"])

        assert outcome.text is None
        assert outcome.note == "git status printed more than 4 bytes"
        assert gitinfo.dirty_count(root) is None


class TestBoundedStderr:
    def test_stderr_is_capped_and_keeps_its_oldest_bytes(self, monkeypatch, tmp_path):
        """The note quotes the first line, so the cap has to keep the front of the stream."""
        monkeypatch.setattr(gitinfo, "MAX_STDERR_BYTES", 16)
        real_popen = subprocess.Popen

        def noisy(command, **kwargs):
            script = "import sys; sys.stderr.write('E' * 200000); raise SystemExit(3)"
            return real_popen([sys.executable, "-c", script], **kwargs)

        monkeypatch.setattr(subprocess, "Popen", noisy)
        outcome = gitinfo.run_bounded(tmp_path, ["log"], timeout=30.0, max_output_bytes=4096)

        assert outcome.note == "git log exited 3: " + "E" * 16


class TestWindowsDrainFallback:
    """``select()`` takes sockets and not pipes on Windows, so that branch reads differently.

    The platform cannot be run here, so every test forces the branch and
    fails loudly if the POSIX drain is reached anyway.
    """

    @pytest.fixture(autouse=True)
    def on_windows(self, monkeypatch):
        wrong_branch = "the POSIX drain ran on the Windows branch"

        def unreachable(*args, **kwargs):
            raise AssertionError(wrong_branch)

        monkeypatch.setattr(sys, "platform", "win32")
        monkeypatch.setattr(gitinfo, "_drain_selectors", unreachable)

    def test_a_successful_call_keeps_stdout_whole(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        outcome = gitinfo.run_bounded(
            root, ["rev-parse", "HEAD"], timeout=5.0, max_output_bytes=4096
        )

        assert outcome.returncode == 0
        assert outcome.truncated is False
        assert outcome.text is not None
        assert len(outcome.text.strip()) == 40

    def test_a_nonzero_exit_is_a_note_with_the_first_stderr_line(self, make_git_repo):
        root = make_git_repo(SAMPLE)
        outcome = gitinfo.run_bounded(
            root, ["rev-parse", "--verify", "no-such-ref-zzz"], timeout=5.0, max_output_bytes=4096
        )

        assert outcome.text is None
        assert outcome.note.startswith(f"git rev-parse exited {outcome.returncode}: fatal:")

    def test_the_output_cap_cuts_stdout_and_flags_it(self, make_git_repo, commit_all):
        root = make_git_repo(SAMPLE)
        (root / "a.py").write_text("x = 2\n", encoding="utf-8")
        commit_all(root, "big\n\n" + "y" * 100_000)

        outcome = gitinfo.run_bounded(
            root, ["log", "-1", "--format=%B"], timeout=30.0, max_output_bytes=4096
        )

        assert outcome.truncated is True
        assert outcome.text is not None
        assert len(outcome.text.encode()) == 4096

    def test_a_deadline_kills_a_silent_child(self, monkeypatch, tmp_path):
        real_popen = subprocess.Popen

        def sleeper(command, **kwargs):
            return real_popen([sys.executable, "-c", "import time; time.sleep(30)"], **kwargs)

        monkeypatch.setattr(subprocess, "Popen", sleeper)
        outcome = gitinfo.run_bounded(tmp_path, ["log"], timeout=0.05, max_output_bytes=10)

        assert outcome.text is None
        assert outcome.note == "git log timed out after 0.05s"


class TestNoCallInTheRunnerIsUnbounded:
    def test_a_child_that_closes_its_pipes_and_lingers_is_killed(self, monkeypatch, tmp_path):
        """EOF on both pipes is not proof the child exited, so the reap is bounded too."""
        real_popen = subprocess.Popen
        script = "import os, time; os.close(1); os.close(2); time.sleep(30)"

        def lingering(command, **kwargs):
            return real_popen([sys.executable, "-c", script], **kwargs)

        monkeypatch.setattr(subprocess, "Popen", lingering)
        outcome = gitinfo.run_bounded(tmp_path, ["log"], timeout=1.0, max_output_bytes=4096)

        assert outcome.text is None
        assert outcome.note == "git log timed out after 1.0s"

    def test_a_spawn_that_yields_no_pipes_degrades_with_a_reason(self, monkeypatch, tmp_path):
        class Pipeless(SilentChild):
            def __init__(self, command, **kwargs):
                super().__init__(command, **kwargs)
                self.stdout.close()
                self.stdout = None

        monkeypatch.setattr(subprocess, "Popen", Pipeless)
        outcome = gitinfo.run_bounded(tmp_path, ["log"], timeout=1.0, max_output_bytes=4096)

        assert outcome.text is None
        assert outcome.note == "git log could not be run: no pipes"


class TestPosixDrainIsTheDefault:
    def test_the_communicate_fallback_is_not_used_here(self, monkeypatch, make_git_repo):
        root = make_git_repo(SAMPLE)

        wrong_branch = "the Windows fallback ran on POSIX"

        def unreachable(*args, **kwargs):
            raise AssertionError(wrong_branch)

        monkeypatch.setattr(gitinfo, "_drain_communicate", unreachable)
        outcome = gitinfo.run_bounded(
            root, ["rev-parse", "HEAD"], timeout=5.0, max_output_bytes=4096
        )

        assert outcome.returncode == 0
