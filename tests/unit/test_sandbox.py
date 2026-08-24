"""Worktree lifecycle and the bounded runner.

The whole security posture of the write side rests on two facts, so both are
asserted directly rather than inferred: ``git status --porcelain`` and
``rev-parse HEAD`` are compared before and after, and the scratch directory is
checked to be gone -- including when the body of the context manager raised,
which is the case a ``finally`` exists for and the one nobody notices is
broken until a cache directory fills up.

The runner's tests are about the same kind of guarantee one layer down: a
command that hangs must come back as a *timeout* within the bound, and the
process group it started must actually be dead afterwards. The hang case is
built to fail a leader-only kill -- the grandchild ignores SIGTERM -- so the
assertion is about the group, not about the one process we hold a handle to.
"""

import errno
import logging
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

import pytest

from agentless_mcp.core import sandbox
from agentless_mcp.core.sandbox import RunStatus
from agentless_mcp.util import cachedir, platforms
from agentless_mcp.util.errors import OperationFailed, RepoResolutionError

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

    def test_a_scratch_root_inside_the_repository_is_refused(self, repo, monkeypatch):
        """Keyed on the property, not on the one way of breaking it.

        `cache_root` now refuses a relative XDG_CACHE_HOME, which was how the
        audit reached this. An operator pointing the variable at a directory
        inside the repository reaches the same place by a route no
        environment rule can catch, so the guard sits where the invariant is:
        the scratch is never inside the target.
        """
        monkeypatch.setenv(cachedir.ENV_CACHE_HOME, str(repo / "inside"))

        with (
            pytest.raises(RepoResolutionError, match="inside the repository"),
            sandbox.worktree(repo),
        ):
            pass

        assert not (repo / "inside").exists(), "the refused location was created anyway"

    def test_a_scratch_root_symlinked_into_the_repository_is_refused(
        self, repo, tmp_path, monkeypatch
    ):
        """The same property, reached by a spelling the lexical test misses.

        Reproduced during the audit: the guard compared an unresolved scratch
        against a resolved `top`, so an XDG_CACHE_HOME that is a symlink into
        the repository read as outside it and the worktree was created inside
        the repository under analysis. Both sides are real paths now, and this
        is the case that says so.
        """
        inside = repo / "inside"
        inside.mkdir()
        cache_home = tmp_path / "cache-home-link"
        cache_home.symlink_to(inside)
        monkeypatch.setenv(cachedir.ENV_CACHE_HOME, str(cache_home))

        with (
            pytest.raises(RepoResolutionError, match="inside the repository"),
            sandbox.worktree(repo),
        ):
            pass

        assert list(inside.iterdir()) == [], "the refused worktree was created anyway"

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

    def test_a_root_that_head_does_not_have_names_the_directory(self, repo):
        """An untracked directory is simply not in a worktree of HEAD.

        The yielded path used to be handed back not existing, and the caller's
        own ``Popen`` was what failed -- reporting that it could not start the
        interpreter, which names neither the missing directory nor the reason
        it is missing.
        """
        untracked = repo / "scratchpad"
        untracked.mkdir()

        def enter():
            with sandbox.worktree(untracked):
                pytest.fail("the body must not run for a directory HEAD does not have")

        with pytest.raises(RepoResolutionError, match="scratchpad"):
            enter()

        scratch = sandbox.scratch_root()
        assert not scratch.is_dir() or list(scratch.iterdir()) == [], (
            "the refused worktree was left behind"
        )

    def test_the_scratch_root_is_under_the_cache_home(self, isolated_cache_home):
        assert sandbox.scratch_root().parent == cachedir.cache_root()
        assert isolated_cache_home in sandbox.scratch_root().parents


class TestWorktreeRunsNoRepositoryCode:
    """Creating a worktree must not execute the analysed repository's code."""

    def hook(self, repo, marker, *, directory=".git/hooks"):
        """Install a ``post-checkout`` hook that writes ``marker``."""
        hooks = repo / directory
        hooks.mkdir(parents=True, exist_ok=True)
        script = hooks / "post-checkout"
        script.write_text(
            f'#!/bin/sh\necho fired > "{marker}"\n',
            encoding="utf-8",
        )
        script.chmod(0o755)
        return script

    def test_a_hook_that_a_plain_checkout_runs_is_not_run(self, repo, tmp_path):
        """The control half proves the hook fires at all, so this is not vacuous."""
        marker = tmp_path / "hook-fired.txt"
        self.hook(repo, marker)

        control = tmp_path / "control-worktree"
        git(repo, "worktree", "add", "--detach", str(control), "HEAD")
        try:
            assert marker.exists(), "the fixture hook never fired; the test proves nothing"
        finally:
            git(repo, "worktree", "remove", "--force", str(control))
        marker.unlink()

        with sandbox.worktree(repo) as tree:
            assert (tree / "app.py").exists()

        assert not marker.exists(), "git worktree add ran the repository's post-checkout hook"

    def test_a_configured_hooks_path_is_neutralised_too(self, repo, tmp_path):
        """A repository can point ``core.hooksPath`` at a directory it ships."""
        marker = tmp_path / "hook-fired.txt"
        self.hook(repo, marker, directory="githooks")
        git(repo, "config", "core.hooksPath", "githooks")

        with sandbox.worktree(repo) as tree:
            assert (tree / "app.py").exists()

        assert not marker.exists(), "core.hooksPath from the repository decided what ran"


class TestWorktreeCreationFailure:
    """A creation that dies part-way leaves nothing in the analysed repository."""

    def records(self, repo):
        directory = repo / ".git" / "worktrees"
        return sorted(entry.name for entry in directory.iterdir()) if directory.is_dir() else []

    def test_a_killed_creation_leaves_no_locked_record_or_scratch(self, repo, monkeypatch):
        real = sandbox.run_git

        def dies_after_the_record_exists(root, arguments, **keywords):
            """Create the worktree for real, lock it, then fail like a kill would."""
            output = real(root, arguments, **keywords)
            if list(arguments[:2]) != ["worktree", "add"]:
                return output
            created = Path(arguments[3])
            (root / ".git" / "worktrees" / created.name / "locked").write_text(
                "killed mid-checkout\n", encoding="utf-8"
            )
            message = "git worktree add timed out"
            raise OperationFailed(message)

        monkeypatch.setattr(sandbox, "run_git", dies_after_the_record_exists)

        def enter():
            with sandbox.worktree(repo):
                pytest.fail("the body must not run when creation failed")

        with pytest.raises(OperationFailed, match="timed out"):
            enter()

        assert self.records(repo) == []
        assert "wt-" not in git(repo, "worktree", "list")
        scratch = sandbox.scratch_root()
        assert not scratch.is_dir() or list(scratch.iterdir()) == []


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
    """The four ways one git call can fail, each with its own message.

    Every one of these is a path the write side takes when something has
    already gone wrong, and the message is the only thing an operator gets.
    The subprocess is stubbed rather than provoked: uninstalling git, hanging
    it, or exhausting file descriptors are not things a unit test may do to
    the machine it runs on.
    """

    def test_a_failing_command_raises_with_the_reason(self, repo):
        with pytest.raises(OperationFailed, match="exited"):
            sandbox.run_git(repo, ["rev-parse", "refs/heads/no-such-branch"])

    def test_a_missing_git_binary_says_git_is_not_installed(self, repo, monkeypatch):
        def absent(command, **keywords):
            raise FileNotFoundError(errno.ENOENT, "No such file or directory", "git")

        monkeypatch.setattr(sandbox.subprocess, "run", absent)

        with pytest.raises(OperationFailed, match="git is not installed"):
            sandbox.run_git(repo, ["status"])

    def test_a_hung_git_reports_the_bound_it_exceeded(self, repo, monkeypatch):
        def hangs(command, **keywords):
            raise subprocess.TimeoutExpired(command, keywords["timeout"])

        monkeypatch.setattr(sandbox.subprocess, "run", hangs)

        with pytest.raises(OperationFailed, match=r"git status timed out after 7\.0s"):
            sandbox.run_git(repo, ["status"], timeout=7.0)

    def test_an_os_error_names_the_subcommand_and_the_directory(self, repo, monkeypatch):
        def refuses(command, **keywords):
            raise OSError(errno.EMFILE, "Too many open files")

        monkeypatch.setattr(sandbox.subprocess, "run", refuses)

        with pytest.raises(OperationFailed, match="git status could not be run"):
            sandbox.run_git(repo, ["status"])

    def test_the_default_bound_is_the_creation_bound(self, repo, monkeypatch):
        seen = []

        def record(command, **keywords):
            seen.append(keywords["timeout"])
            raise subprocess.TimeoutExpired(command, keywords["timeout"])

        monkeypatch.setattr(sandbox.subprocess, "run", record)

        with pytest.raises(OperationFailed):
            sandbox.run_git(repo, ["status"])

        assert seen == [sandbox.GIT_TIMEOUT_SECONDS]


class TestRelease:
    """The cleanup ladder: git, then rmtree, then a line naming the leftover.

    `_release` runs from a ``finally`` that usually covers a caller who is
    already failing, so nothing in it may raise. Each rung is exercised on its
    own, because the whole point of a ladder is what happens when a rung
    breaks.
    """

    @pytest.fixture
    def scratch(self, tmp_path):
        directory = tmp_path / "wt-fixture"
        directory.mkdir()
        (directory / "app.py").write_text(FILES["app.py"], encoding="utf-8")
        return directory

    def refusing_git(self, *, failing):
        """A ``run_git`` stub that fails for the named subcommands."""
        calls = []

        def run(root, arguments, **keywords):
            calls.append((tuple(arguments), keywords.get("timeout")))
            if arguments[1] in failing:
                message = f"git worktree {arguments[1]} exited 128"
                raise OperationFailed(message)
            return ""

        return run, calls

    def test_a_failed_remove_falls_back_to_deleting_the_directory(
        self, tmp_path, scratch, monkeypatch, caplog
    ):
        run, _calls = self.refusing_git(failing={"remove"})
        monkeypatch.setattr(sandbox, "run_git", run)

        with caplog.at_level(logging.WARNING, logger=sandbox.logger.name):
            sandbox._release(tmp_path, scratch)

        assert not scratch.exists()
        assert "git worktree remove failed" in caplog.text

    def test_a_failed_prune_is_logged_and_not_raised(self, tmp_path, scratch, monkeypatch, caplog):
        run, _calls = self.refusing_git(failing={"prune"})
        monkeypatch.setattr(sandbox, "run_git", run)

        with caplog.at_level(logging.WARNING, logger=sandbox.logger.name):
            sandbox._release(tmp_path, scratch)

        assert "git worktree prune failed" in caplog.text

    def test_a_leftover_directory_is_named_for_an_operator_to_delete(
        self, tmp_path, scratch, monkeypatch, caplog
    ):
        """Both rungs break: git refuses and the tree cannot be removed."""
        run, _calls = self.refusing_git(failing={"remove", "prune"})
        monkeypatch.setattr(sandbox, "run_git", run)
        monkeypatch.setattr(sandbox.shutil, "rmtree", lambda path, ignore_errors=False: None)

        with caplog.at_level(logging.ERROR, logger=sandbox.logger.name):
            sandbox._release(tmp_path, scratch)

        assert scratch.exists()
        assert str(scratch) in caplog.text
        assert "delete it by hand" in caplog.text

    def test_both_cleanup_calls_take_the_shorter_bound(self, tmp_path, scratch, monkeypatch):
        """A stuck cleanup holds `_WORKTREE_LOCK`, so its bound is the pool's wait."""
        run, calls = self.refusing_git(failing=set())
        monkeypatch.setattr(sandbox, "run_git", run)

        sandbox._release(tmp_path, scratch)

        assert [timeout for _arguments, timeout in calls] == [
            sandbox.GIT_CLEANUP_TIMEOUT_SECONDS,
            sandbox.GIT_CLEANUP_TIMEOUT_SECONDS,
        ]
        assert sandbox.GIT_CLEANUP_TIMEOUT_SECONDS < sandbox.GIT_TIMEOUT_SECONDS


# A script that outlives any bound, records the process group it leads, and
# leaves behind a child that ignores SIGTERM. Killing only the direct child
# would leave that grandchild running, which is exactly the orphan the
# process-group kill exists to prevent -- so the test can tell the two apart.
HANG_SCRIPT = """\
import os
import subprocess
import sys
import time

stubborn = (
    "import signal, time\\n"
    "signal.signal(signal.SIGTERM, signal.SIG_IGN)\\n"
    "time.sleep(600)\\n"
)
child = subprocess.Popen([sys.executable, "-c", stubborn])
with open(sys.argv[1], "w", encoding="utf-8") as handle:
    handle.write(f"{os.getpgrp()} {child.pid}\\n")
    handle.flush()
time.sleep(600)
"""

# A leader that ignores SIGTERM outright, so cleanup pays the full grace
# before SIGKILL -- the worst case the documented wall-clock bound is made of:
# timeout + TERM_GRACE_SECONDS + KILL_REAP_SECONDS.
STUBBORN_LEADER_SCRIPT = """\
import signal
import time

signal.signal(signal.SIGTERM, signal.SIG_IGN)
time.sleep(600)
"""

CHATTY_SCRIPT = """\
for number in range(2000):
    print(f"line {number:05d}")
"""

FAILING_SCRIPT = """\
import sys

sys.stderr.write("the suite is red\\n")
raise SystemExit(3)
"""

# A runaway writer: two megabytes as fast as the disk takes them, then a pause
# long enough for the runner's wait loop to look at the file, then a report of
# how large its own stdout still is. The command asks the kernel rather than
# the test asking it, because the capture file belongs to the runner and is
# gone by the time the result comes back.
FLOOD_SCRIPT = """\
import os
import sys
import time

block = "x" * 8192
for _ in range(256):
    sys.stdout.write(block)
sys.stdout.flush()
time.sleep(2)
sys.stderr.write(f"held={os.fstat(sys.stdout.fileno()).st_size}\\n")
sys.stdout.write("tail-marker\\n")
sys.stdout.flush()
"""


def process_is_gone(pid, deadline=15.0):
    """Poll until ``pid`` has exited, or give up.

    The child PID is more precise than probing a numeric process-group ID after
    cleanup: BSD can reuse that ID for a process owned by another user and
    report ``EPERM`` even though the original child is gone.
    """
    stop = time.monotonic() + deadline
    while time.monotonic() < stop:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        except PermissionError:
            # This script never changes identity, so a foreign owner means
            # the original PID was already reaped and reused.
            return True
        time.sleep(0.05)
    return False


class TestRunCommand:
    @pytest.fixture
    def workspace(self, tmp_path):
        directory = tmp_path / "workspace"
        directory.mkdir()
        return directory

    def write(self, workspace, name, body):
        (workspace / name).write_text(body, encoding="utf-8")
        return name

    def test_a_zero_exit_is_a_pass(self, workspace, python_cmd):
        self.write(workspace, "ok.py", "print('fine')\n")
        result = sandbox.run_command(workspace, python_cmd("ok.py"), timeout=30)

        assert result.status is RunStatus.PASSED
        assert result.passed
        assert result.exit_code == 0
        assert "fine" in result.stdout_tail

    def test_a_non_zero_exit_is_a_failure_carrying_stderr(self, workspace, python_cmd):
        self.write(workspace, "red.py", FAILING_SCRIPT)
        result = sandbox.run_command(workspace, python_cmd("red.py"), timeout=30)

        assert result.status is RunStatus.FAILED
        assert not result.passed
        assert result.exit_code == 3
        assert "the suite is red" in result.stderr_tail

    def test_the_command_runs_in_the_directory_it_was_given(self, workspace, python_cmd):
        self.write(workspace, "here.py", "import pathlib\nprint(pathlib.Path.cwd())\n")
        result = sandbox.run_command(workspace, python_cmd("here.py"), timeout=30)

        assert result.stdout_tail.strip() == str(workspace.resolve())

    def test_parent_environment_is_absent_unless_explicitly_passed_through(
        self, workspace, python_cmd, monkeypatch
    ):
        marker = "AGENTLESS_MCP_TEST_SECRET"
        monkeypatch.setenv(marker, "contained-value")
        self.write(
            workspace,
            "environment.py",
            f"import os\nprint(os.environ.get({marker!r}, 'absent'))\n",
        )

        scrubbed = sandbox.run_command(workspace, python_cmd("environment.py"), timeout=30)
        passed = sandbox.run_command(
            workspace,
            python_cmd("environment.py"),
            timeout=30,
            passthrough_env=(marker,),
        )

        assert scrubbed.stdout_tail.strip() == "absent"
        assert passed.stdout_tail.strip() == "contained-value"

    def test_a_hang_is_a_timeout_not_a_pass(self, workspace, python_cmd):
        marker = workspace / "pgid.txt"
        self.write(workspace, "hang.py", HANG_SCRIPT)

        started = time.monotonic()
        result = sandbox.run_command(workspace, python_cmd("hang.py", str(marker)), timeout=1)
        elapsed = time.monotonic() - started

        assert result.status is RunStatus.TIMEOUT
        assert not result.passed
        # A timeout has no exit code: the number a killed process reports is
        # the signal, and reading it as a status is how a hang becomes an
        # ordinary failure.
        assert result.exit_code is None
        assert elapsed < 1 + sandbox.TERM_GRACE_SECONDS + sandbox.KILL_REAP_SECONDS + 10

    def test_a_sigterm_ignoring_command_stays_inside_the_documented_bound(
        self, workspace, python_cmd
    ):
        self.write(workspace, "stubborn.py", STUBBORN_LEADER_SCRIPT)

        started = time.monotonic()
        result = sandbox.run_command(workspace, python_cmd("stubborn.py"), timeout=1)
        elapsed = time.monotonic() - started

        assert result.status is RunStatus.TIMEOUT
        # The documented worst case: the timeout, the full SIGTERM grace this
        # command insists on consuming, then the short reap wait after SIGKILL.
        # The margin covers interpreter start-up on a slow runner; it is well
        # under the second full grace the old cleanup could burn.
        assert elapsed < 1 + sandbox.TERM_GRACE_SECONDS + sandbox.KILL_REAP_SECONDS + 3

    def test_the_whole_process_group_is_dead_afterwards(self, workspace, python_cmd):
        marker = workspace / "pgid.txt"
        self.write(workspace, "hang.py", HANG_SCRIPT)

        result = sandbox.run_command(workspace, python_cmd("hang.py", str(marker)), timeout=2)

        assert result.status is RunStatus.TIMEOUT
        # Checked before unpacking: on a loaded runner the script may not
        # reach its write inside the bound, and an unpack of a short list
        # fails with a ValueError that names none of that.
        recorded = marker.read_text(encoding="utf-8").split()
        assert len(recorded) == 2, f"hang.py recorded {recorded!r}, not a pgid and a child pid"
        _group, child = (int(value) for value in recorded)
        assert process_is_gone(child), f"child process {child} outlived the run"

    def test_an_interrupted_run_still_ends_the_process_group(
        self, workspace, python_cmd, monkeypatch
    ):
        """Ctrl-C reaches the server, never the command the server started.

        `start_new_session=True` is what makes the group signalable, and it
        is the same thing that takes the child out of the terminal's
        foreground group. So the one event most likely to end a run is the
        one event that used to leave the command running -- holding the port
        or the lock the next run needs. Raised from inside the wait rather
        than sent for real, so the test does not depend on this process's own
        signal disposition.
        """
        marker = workspace / "pgid.txt"
        self.write(workspace, "hang.py", HANG_SCRIPT)

        real = sandbox._wait_bounded

        def interrupt(process, streams, *, timeout, capture):
            # Let the child reach the point where it has recorded its pids,
            # then interrupt the parent exactly as an operator would.
            real(process, streams, timeout=2, capture=capture)
            raise KeyboardInterrupt

        monkeypatch.setattr(sandbox, "_wait_bounded", interrupt)

        with pytest.raises(KeyboardInterrupt):
            sandbox.run_command(workspace, python_cmd("hang.py", str(marker)), timeout=30)

        _group, child = (int(value) for value in marker.read_text(encoding="utf-8").split())
        assert process_is_gone(child), f"child process {child} outlived the interrupt"

    def test_ending_an_already_reaped_process_does_not_raise(self, workspace, python_cmd):
        """Cleanup now runs on paths where the process may already be gone.

        `_kill_group` signals unconditionally rather than checking liveness
        first, because a check-then-signal is a race. That only works if the
        signal tolerates a group that has already exited, so the tolerance is
        the thing to pin.
        """
        self.write(workspace, "ok.py", "print('fine')\n")
        result = sandbox.run_command(workspace, python_cmd("ok.py"), timeout=30)
        assert result.status is RunStatus.PASSED

        process = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
        process.wait()

        sandbox._kill_group(process, platforms.family(sys.platform))

    def test_a_group_that_cannot_be_signalled_is_logged_not_raised(self, monkeypatch, caplog):
        """A permission error during cleanup must not become the run's failure.

        Signalling can be refused when a group id has been reused by another
        user's process. The command already failed or was interrupted at that
        point; turning cleanup into an exception replaces a reported outcome
        with a traceback.
        """

        def refuse(group, number):
            message = "not permitted"
            raise PermissionError(message)

        monkeypatch.setattr(sandbox.os, "killpg", refuse)

        with caplog.at_level(logging.ERROR, logger=sandbox.logger.name):
            sandbox._signal_group(4_000_000, signal.SIGTERM)

        assert "not permitted to signal process group" in caplog.text

    def test_the_capture_keeps_the_tail(self, workspace, python_cmd):
        self.write(workspace, "chatty.py", CHATTY_SCRIPT)
        result = sandbox.run_command(
            workspace, python_cmd("chatty.py"), timeout=30, max_capture=200
        )

        assert result.status is RunStatus.PASSED
        assert "line 01999" in result.stdout_tail
        assert "line 00000" not in result.stdout_tail
        assert "earlier bytes dropped" in result.stdout_tail

    def test_a_short_output_is_not_marked_as_truncated(self, workspace, python_cmd):
        self.write(workspace, "ok.py", "print('brief')\n")
        result = sandbox.run_command(workspace, python_cmd("ok.py"), timeout=30, max_capture=200)

        assert result.stdout_tail == "brief\n"

    def test_the_capture_is_bounded_while_it_is_being_written(self, workspace, python_cmd):
        """``max_capture`` bounds the file on disk, not only the tail read back."""
        self.write(workspace, "flood.py", FLOOD_SCRIPT)
        result = sandbox.run_command(
            workspace, python_cmd("flood.py"), timeout=30, max_capture=1000
        )

        assert result.status is RunStatus.PASSED
        held = int(result.stderr_tail.split("held=")[1].split()[0])
        assert held <= 1000 * sandbox.CAPTURE_SLACK, (
            f"the command left {held} bytes on disk from a 1000-byte cap"
        )

    def test_the_tail_after_a_trim_is_output_and_not_padding(self, workspace, python_cmd):
        """What the child writes after a trim is still reported, without the hole."""
        self.write(workspace, "flood.py", FLOOD_SCRIPT)
        result = sandbox.run_command(
            workspace, python_cmd("flood.py"), timeout=30, max_capture=1000
        )

        assert "tail-marker" in result.stdout_tail
        assert "\x00" not in result.stdout_tail
        assert "earlier bytes dropped" in result.stdout_tail

    def test_a_command_that_cannot_be_started_is_an_error(self, workspace):
        result = sandbox.run_command(workspace, "./no-such-binary-9f2c", timeout=30)

        assert result.status is RunStatus.ERROR
        assert result.exit_code is None
        assert "no-such-binary-9f2c" in result.stderr_tail

    def test_an_unsplittable_command_is_an_error(self, workspace):
        result = sandbox.run_command(workspace, 'echo "unbalanced', timeout=30)

        assert result.status is RunStatus.ERROR
        assert "argv" in result.stderr_tail

    def test_an_empty_command_is_an_error(self, workspace):
        result = sandbox.run_command(workspace, "   ", timeout=30)

        assert result.status is RunStatus.ERROR
        assert "empty" in result.stderr_tail

    def test_the_command_is_never_interpreted_by_a_shell(self, workspace, python_cmd):
        """A metacharacter is an argument, not a second statement."""
        self.write(workspace, "argv.py", "import sys\nprint(len(sys.argv))\n")
        canary = workspace / "canary.txt"
        command = f"{python_cmd('argv.py')} ; touch {canary}"

        result = sandbox.run_command(workspace, command, timeout=30)

        assert result.status is RunStatus.PASSED
        assert result.stdout_tail.strip() == "4"
        assert not canary.exists()

    def test_stdin_is_closed_so_a_prompt_cannot_hang_forever(self, workspace, python_cmd):
        self.write(workspace, "ask.py", "print(repr(input()))\n")
        result = sandbox.run_command(workspace, python_cmd("ask.py"), timeout=30)

        assert result.status is RunStatus.FAILED
        assert "EOFError" in result.stderr_tail


class TestEnvironmentAllowlist:
    """Which parent variables a child inherits is a per-platform decision.

    Tested as the dispatch it is, for the reason ``test_platform_dispatch``
    gives: the choice is a pure function of the family, so both branches are
    provable on this platform even though only one of them ever runs here.
    """

    def test_each_family_gets_its_own_names(self):
        assert sandbox._env_allowlist(platforms.POSIX) == sandbox.POSIX_TEST_ENV_ALLOWLIST
        assert sandbox._env_allowlist(platforms.WINDOWS) == sandbox.WINDOWS_TEST_ENV_ALLOWLIST

    def test_the_windows_list_names_what_a_cpython_child_needs_to_start(self):
        # A child given no SystemRoot fails during interpreter start-up, so a
        # POSIX-only allowlist meant no Python suite could run on the platform
        # the process-group code was written for.
        for name in ("PATH", "PATHEXT", "SYSTEMROOT", "COMSPEC", "TEMP"):
            assert name in sandbox.WINDOWS_TEST_ENV_ALLOWLIST

    def test_the_windows_environment_holds_windows_names_and_not_posix_ones(self, monkeypatch):
        monkeypatch.setenv("SYSTEMROOT", "C:\\Windows")
        monkeypatch.setenv("HOME", "/home/somebody")

        built = sandbox._test_environment((), platforms.WINDOWS)

        assert built["SYSTEMROOT"] == "C:\\Windows"
        assert "HOME" not in built, "a POSIX-only name reached a Windows child"

    def test_an_absent_name_stays_absent_rather_than_being_invented(self, monkeypatch):
        monkeypatch.delenv("LANG", raising=False)
        assert "LANG" not in sandbox._test_environment((), platforms.POSIX)

    @pytest.mark.parametrize("flavour", [platforms.POSIX, platforms.WINDOWS])
    def test_a_passthrough_name_is_added_on_either_family(self, monkeypatch, flavour):
        monkeypatch.setenv("AGENTLESS_MCP_TEST_EXTRA", "value")
        built = sandbox._test_environment(("AGENTLESS_MCP_TEST_EXTRA",), flavour)
        assert built["AGENTLESS_MCP_TEST_EXTRA"] == "value"


class _StubProcess:
    """A ``Popen`` stand-in for the kill paths, which touch four members.

    A stub rather than a real process, because the branches under test are the
    ones a real process cannot be made to take on demand: a leader that
    survives ``kill()`` is stuck in the kernel, and a suite may not arrange
    that on the machine it runs on.
    """

    def __init__(self, *, dies_on=()):
        self.pid = 4242
        self.returncode = None
        self.calls = []
        self._dies_on = dies_on

    def terminate(self):
        self.calls.append("terminate")
        self._maybe_die("terminate")

    def kill(self):
        self.calls.append("kill")
        self._maybe_die("kill")

    def wait(self, timeout=None):
        self.calls.append("wait")
        if self.returncode is None:
            command = "stub"
            raise subprocess.TimeoutExpired(command, timeout)
        return self.returncode

    def _maybe_die(self, event):
        if event in self._dies_on:
            self.returncode = 0


class TestKillLeader:
    """The Windows cleanup path, which no test on this platform ever reaches."""

    def test_a_leader_that_exits_on_terminate_is_never_killed(self):
        process = _StubProcess(dies_on=("terminate",))

        sandbox._kill_leader(process)

        assert process.calls == ["terminate", "wait"]

    def test_a_leader_that_ignores_terminate_is_killed(self, caplog):
        process = _StubProcess(dies_on=("kill",))

        with caplog.at_level(logging.INFO, logger=sandbox.logger.name):
            sandbox._kill_leader(process)

        assert process.calls == ["terminate", "wait", "kill", "wait"]
        assert "ignored terminate()" in caplog.text

    def test_a_leader_that_survives_kill_is_reported_not_raised(self, caplog):
        process = _StubProcess()

        with caplog.at_level(logging.ERROR, logger=sandbox.logger.name):
            sandbox._kill_leader(process)

        assert "still present after kill()" in caplog.text


class TestKillGroupEscalation:
    """The POSIX ladder, including the rung that means the kernel is stuck."""

    def test_a_group_that_survives_sigkill_is_reported_not_raised(self, monkeypatch, caplog):
        signalled = []
        monkeypatch.setattr(
            sandbox.os, "killpg", lambda group, number: signalled.append((group, number))
        )
        process = _StubProcess()

        with caplog.at_level(logging.ERROR, logger=sandbox.logger.name):
            sandbox._kill_group(process, platforms.POSIX)

        assert signalled == [
            (process.pid, signal.SIGTERM),
            (process.pid, signal.SIGKILL),
        ]
        assert "still present after SIGKILL" in caplog.text

    def test_sigkill_follows_sigterm_even_when_the_leader_exited(self, monkeypatch):
        """The leader dying says nothing about the children it spawned."""
        signalled = []
        monkeypatch.setattr(sandbox.os, "killpg", lambda group, number: signalled.append(number))
        process = _StubProcess(dies_on=("terminate",))
        process.returncode = 0

        sandbox._kill_group(process, platforms.POSIX)

        assert signalled == [signal.SIGTERM, signal.SIGKILL]


class TestTrim:
    """``_trim`` judges the live bytes, not the file size the hole inflates.

    The unit is exercised directly rather than through a live command: the bug
    it fixes is a race between one poll tick and the child's final write, and
    reproducing that timing from a subprocess is exactly the flaky test this
    machinery exists to avoid. What can be pinned deterministically is the
    decision -- once a file has been truncated, ``st_size`` still counts the
    hole, and a trim that compares raw size truncates every later poll and eats
    the tail the runner is about to report.
    """

    def test_a_file_over_the_ceiling_is_emptied_and_reports_its_hole(self, tmp_path):
        path = tmp_path / "capture.log"
        with path.open("wb") as handle:
            handle.write(b"x" * 300)
            handle.flush()

            hole = sandbox._trim(handle, 100, 0)

        assert hole == 300
        assert path.stat().st_size == 0

    def test_a_short_tail_written_past_the_hole_survives_the_next_poll(self, tmp_path):
        path = tmp_path / "capture.log"
        with path.open("wb") as handle:
            handle.write(b"x" * 300)
            handle.flush()
            hole = sandbox._trim(handle, 100, 0)

            # What the child writes next lands at its own offset, past the end
            # of the emptied file: 4 live bytes in a file st_size still calls
            # 304. Judged on raw size that is over the ceiling and the tail is
            # truncated away; judged on live bytes it is not.
            handle.seek(hole)
            handle.write(b"tail")
            handle.flush()

            unchanged = sandbox._trim(handle, 100, hole)

        assert unchanged == hole
        assert path.stat().st_size == hole + len(b"tail")

    def test_live_bytes_past_the_ceiling_trim_again_from_the_new_hole(self, tmp_path):
        path = tmp_path / "capture.log"
        with path.open("wb") as handle:
            handle.write(b"x" * 300)
            handle.flush()
            first = sandbox._trim(handle, 100, 0)

            handle.seek(first)
            handle.write(b"y" * 200)
            handle.flush()

            second = sandbox._trim(handle, 100, first)

        assert second == first + 200
        assert path.stat().st_size == 0
