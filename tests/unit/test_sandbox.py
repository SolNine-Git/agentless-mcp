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

import os
import subprocess
import time

import pytest

from agentless_mcp.core import cache, sandbox
from agentless_mcp.core.sandbox import RunStatus
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
    handle.write(f"{os.getpgrp()}\\n")
    handle.flush()
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


def group_is_gone(group, deadline=15.0):
    """Poll until no process remains in ``group``, or give up.

    A poll rather than a single probe: SIGKILL delivery and the reaping of an
    orphan by init are both asynchronous, so a one-shot check would be a race
    that fails on a loaded machine and passes on an idle one.
    """
    stop = time.monotonic() + deadline
    while time.monotonic() < stop:
        try:
            os.killpg(group, 0)
        except ProcessLookupError:
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
        assert elapsed < 1 + sandbox.TERM_GRACE_SECONDS * 2 + 10

    def test_the_whole_process_group_is_dead_afterwards(self, workspace, python_cmd):
        marker = workspace / "pgid.txt"
        self.write(workspace, "hang.py", HANG_SCRIPT)

        result = sandbox.run_command(workspace, python_cmd("hang.py", str(marker)), timeout=2)

        assert result.status is RunStatus.TIMEOUT
        group = int(marker.read_text(encoding="utf-8").strip())
        assert group_is_gone(group), f"process group {group} outlived the run"

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
