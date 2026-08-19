"""Worktree lifecycle and the bounded test runner behind ``validate``.

Two things live here, both of them the write side's contact with the outside
world: materialising a throwaway checkout, and running one caller-supplied
command inside it under a hard bound.

The read surface of this package is strictly read-only toward an analysed
repository, and the write surface keeps that posture by default: applying a
patch materialises a detached ``git worktree`` under the user's cache
directory, writes there, and produces the diff from there. The caller's
checkout -- including any work in progress in it -- is never opened for
writing, which the tests assert by comparing ``git status --porcelain`` and
``HEAD`` before and after.

Scratch worktrees live under ``$XDG_CACHE_HOME/agentless-mcp/worktrees``,
never inside the target repository. A scratch directory inside the tree under
analysis would show up in that tree's own status, its own walk and its own
gitignore decisions, which is the same class of mistake as writing a cache
into somebody else's repo.

Removal happens in a ``finally``, so a patch that raises mid-apply still
leaves nothing behind. Cleanup failures are logged with the path rather than
raised: raising from the ``finally`` would replace whatever the caller was
already failing with, and losing the real error to report a leftover directory
is the wrong trade. The log line names the directory an operator can delete.

Every git call is a fixed argv with a timeout and no shell. Nothing derived
from repository content or from patch text reaches it.

The test runner takes the opposite kind of input -- a command string the
*caller* supplied -- and keeps the same posture. It is split with ``shlex``
and executed as an argv, never through a shell, so a command carrying ``;`` or
``$(...)`` is one argument rather than two statements. It runs in its own
session, so the whole process group can be killed when the bound expires:
a test suite that spawns a server and hangs must leave nothing running.

The bound is hard, and a command that hits it is a **failure**. A hung test
run is the case this machinery exists to catch, and the one place where
"we did not find out" must never render as green.

Process-group control is where the platforms genuinely differ. POSIX gets the
full guarantee: a new session, then SIGTERM and SIGKILL to the whole group, so
grandchildren die with their parent. Windows gets a documented best effort --
a new process group at spawn, then ``terminate()`` and ``kill()`` on the
leader -- and that difference is stated in the README rather than papered
over, because a caller who believes stray children are impossible on a
platform where they are not has been told something false.
"""

import logging
import os
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import IO, Any

from agentless_mcp.core import cache, gitinfo
from agentless_mcp.util import platforms
from agentless_mcp.util.errors import AtlasError, RepoResolutionError

logger = logging.getLogger(__name__)

# Creating a worktree copies HEAD out; on a large repository that is slower
# than a status read but nowhere near a minute. A bound that can be hit by a
# healthy repository is a flake, and no bound at all is a hang.
GIT_TIMEOUT_SECONDS = 120.0

WORKTREE_DIR = "worktrees"

# How long a timed-out process group gets between SIGTERM and SIGKILL. Long
# enough for a test runner to flush its own summary, short enough that the
# caller's timeout stays a bound they can reason about.
TERM_GRACE_SECONDS = 5.0

# The default cap on captured output, per stream. The tail is what is kept:
# a failing test run puts its summary at the end, and the megabyte of
# progress output before it is the part nobody reads.
DEFAULT_MAX_CAPTURE = 100_000

# ``git worktree add`` and ``git worktree prune`` race each other: prune walks
# the repository's worktree records and can remove one that a concurrent add
# has created but not yet populated. Serialising the bookkeeping within this
# process is what lets `validate --jobs N` give every candidate its own
# worktree. The lock is held across the git call only, never across the body
# of the context manager, so nested worktrees do not deadlock.
_WORKTREE_LOCK = threading.Lock()


class RunStatus(str, Enum):
    """What became of one bounded command.

    ``str, Enum`` rather than ``StrEnum`` for the 3.10 floor, matching
    :class:`agentless_mcp.core.patches.EditStatus`.
    """

    PASSED = "passed"
    FAILED = "failed"
    TIMEOUT = "timeout"
    ERROR = "error"

    def __str__(self) -> str:
        """Return the member value, matching ``enum.StrEnum`` semantics."""
        return self.value


@dataclass(frozen=True)
class RunResult:
    """One command's outcome: its status, its exit code and the tail of its output.

    ``exit_code`` is ``None`` for :data:`RunStatus.TIMEOUT` and
    :data:`RunStatus.ERROR`, because in neither case did the command produce
    one: a killed process reports the signal that killed it, and a command
    that never started reports nothing at all. Reading a signal number as an
    exit status is how a hang becomes an ordinary failure in a report.
    """

    status: RunStatus
    exit_code: int | None
    duration: float
    stdout_tail: str
    stderr_tail: str

    @property
    def passed(self) -> bool:
        """True only when the command ran to completion and exited zero."""
        return self.status is RunStatus.PASSED

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this result, tails included."""
        return {
            "status": self.status.value,
            "exit_code": self.exit_code,
            "duration": self.duration,
            "stdout_tail": self.stdout_tail,
            "stderr_tail": self.stderr_tail,
        }


def scratch_root() -> Path:
    """Return the directory scratch worktrees are created under."""
    return cache.cache_root() / WORKTREE_DIR


@contextmanager
def worktree(root: Path) -> Iterator[Path]:
    """Materialise a detached worktree of ``root`` at HEAD, and remove it after.

    Yields the directory inside the worktree that *corresponds to* ``root``.
    Git only ever creates a worktree of a whole repository, so when ``root`` is
    a subdirectory -- a package inside a monorepo, a fixture directory inside
    a larger checkout -- the yielded path is that same subdirectory within the
    copy. A caller resolving a repository-relative path against it therefore
    lands on the same file it would have in the original, which is the whole
    point of handing back a path rather than a repository.

    Refuses a directory that is not inside a git repository: without HEAD
    there is nothing to detach from, and silently falling back to copying the
    working tree would produce a diff against a base nobody named.
    """
    resolved = root.resolve()
    top = gitinfo.git_root(resolved)
    if top is None:
        message = (
            f"{resolved} is not inside a git repository, so no worktree can be created for it. "
            "Patch apply needs a git repository; run it against one, or use --in-place."
        )
        raise RepoResolutionError(message)

    scratch = scratch_root()
    scratch.mkdir(parents=True, exist_ok=True)
    scratch.chmod(cache.DIRECTORY_MODE)

    path = scratch / f"wt-{uuid.uuid4().hex}"
    with _WORKTREE_LOCK:
        run_git(top, ["worktree", "add", "--detach", str(path), "HEAD"])
    try:
        yield path / resolved.relative_to(top)
    finally:
        with _WORKTREE_LOCK:
            _release(top, path)


def diff(worktree_path: Path) -> str:
    """Return the unified diff of the uncommitted changes in ``worktree_path``.

    ``--no-color`` and ``--no-ext-diff`` are explicit because a worktree reads
    the repository's own configuration: a ``color.diff = always`` or a
    ``diff.external`` in the user's config would otherwise decide the format
    of a diff this tool promises is machine-readable.
    """
    return run_git(worktree_path, ["diff", "--no-color", "--no-ext-diff"])


def run_command(
    cwd: Path,
    cmd: str,
    *,
    timeout: int,
    max_capture: int = DEFAULT_MAX_CAPTURE,
) -> RunResult:
    """Run one caller-supplied command in ``cwd`` under a hard time bound.

    Never raises: every way this can go wrong is a status. A command that
    cannot be parsed or started is :data:`RunStatus.ERROR`, one that outlives
    ``timeout`` seconds is :data:`RunStatus.TIMEOUT`, and only exit zero is
    :data:`RunStatus.PASSED`. A caller reading these four values cannot
    accidentally treat "we never found out" as a pass, which is the whole
    point of the type.

    Three properties are load-bearing rather than incidental:

    * **No shell.** ``shlex.split`` produces an argv and ``Popen`` receives
      it. There is no interpretation of metacharacters at any point.
    * **Own process group.** ``start_new_session=True`` on POSIX makes the
      child a process group leader, so the timeout path can signal the group
      and reach the grandchildren a test runner spawned. Killing only the
      direct child leaves the server it started holding a port. Windows gets
      ``CREATE_NEW_PROCESS_GROUP``, which is the closest thing it has.
    * **Output goes to files, not pipes.** A pipe nobody drains fills at
      64 KB and blocks the writer, turning a chatty passing test into a
      timeout. Temporary files decouple the two, and the tail is read back
      after the process is gone.
    """
    try:
        argv = shlex.split(cmd)
    except ValueError as exc:
        return _spawn_failure(f"command cannot be split into an argv: {exc}")
    if not argv:
        return _spawn_failure("command is empty")

    flavour = platforms.family(sys.platform)
    started = time.monotonic()
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        try:
            # Fixed argv from shlex, no shell, stdin closed: a test command
            # that waits for input is a hang, and a hang is a timeout.
            process = subprocess.Popen(
                argv,
                cwd=cwd,
                stdin=subprocess.DEVNULL,
                stdout=out,
                stderr=err,
                **_group_kwargs(flavour),
            )
        except OSError as exc:
            reason = exc.strerror or str(exc)
            elapsed = time.monotonic() - started
            return _spawn_failure(f"could not start {argv[0]!r}: {reason}", elapsed)

        timed_out = False
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            _kill_group(process, flavour)

        duration = round(time.monotonic() - started, 3)
        stdout_tail = _tail(out, max_capture)
        stderr_tail = _tail(err, max_capture)

    if timed_out:
        return RunResult(RunStatus.TIMEOUT, None, duration, stdout_tail, stderr_tail)

    code = process.returncode
    status = RunStatus.PASSED if code == 0 else RunStatus.FAILED
    return RunResult(status, code, duration, stdout_tail, stderr_tail)


def _spawn_failure(reason: str, duration: float = 0.0) -> RunResult:
    """Return the result of a command that never ran, with the reason on stderr."""
    return RunResult(RunStatus.ERROR, None, round(duration, 3), "", reason)


def _group_kwargs(flavour: str) -> dict[str, Any]:
    """Return the Popen arguments that give the child its own process group.

    ``CREATE_NEW_PROCESS_GROUP`` is read with ``getattr`` because the constant
    does not exist off Windows; the zero default is never used there, since
    the flavour decides which branch runs.
    """
    if flavour == platforms.WINDOWS:
        return {"creationflags": getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)}
    return {"start_new_session": True}


def _kill_group(process: "subprocess.Popen[bytes]", flavour: str) -> None:
    """End the timed-out command, politely then not.

    On POSIX the group id is the child's pid because it was started in a new
    session, so this never has to ask the kernel for a pgid that may already
    be gone. SIGKILL is sent unconditionally after the grace period rather
    than only when the leader is still alive: the leader exiting on SIGTERM
    says nothing about the children it spawned, and those are the orphans that
    outlive a run and hold a port for the next one.

    On Windows only the leader is signalled. That is the best effort the
    platform gives without a job object, and it is stated as such rather than
    presented as the same guarantee.
    """
    if flavour == platforms.WINDOWS:
        _kill_leader(process)
        return

    group = process.pid
    _signal_group(group, signal.SIGTERM)
    try:
        process.wait(timeout=TERM_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        logger.info("process group %d ignored SIGTERM; escalating to SIGKILL", group)

    _signal_group(group, signal.SIGKILL)
    try:
        process.wait(timeout=TERM_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        logger.exception(
            "process group %d is still present after SIGKILL; it may be stuck in the kernel",
            group,
        )


def _kill_leader(process: "subprocess.Popen[bytes]") -> None:
    """End the timed-out command's leader process, politely then not.

    The Windows path. Anything the command spawned survives it, which is why
    the README says the timeout guarantee there is best effort: without a job
    object there is nothing to signal a whole tree with.
    """
    process.terminate()
    try:
        process.wait(timeout=TERM_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        logger.info("process %d ignored terminate(); escalating to kill()", process.pid)
    else:
        return

    process.kill()
    try:
        process.wait(timeout=TERM_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        logger.exception("process %d is still present after kill()", process.pid)


def _signal_group(group: int, number: int) -> None:
    """Send one signal to a process group, tolerating a group that is already gone."""
    try:
        os.killpg(group, number)
    except ProcessLookupError:
        logger.debug("process group %d had already exited before signal %d", group, number)
    except PermissionError:
        logger.exception("not permitted to signal process group %d; it may survive this run", group)


def _tail(handle: IO[bytes], limit: int) -> str:
    """Return the last ``limit`` bytes written to ``handle``, decoded.

    The tail rather than the head, and marked when anything was dropped: the
    end of a test run carries the failure summary, and an unmarked truncation
    would read as the whole story.
    """
    end = handle.seek(0, os.SEEK_END)
    if end == 0:
        return ""

    start = max(0, end - max(limit, 0))
    handle.seek(start)
    text = handle.read().decode("utf-8", errors="replace")
    if start == 0:
        return text
    return f"[... {start} earlier bytes dropped ...]\n{text}"


def run_git(cwd: Path, arguments: Sequence[str]) -> str:
    """Run one bounded git command, raising on anything but success.

    Unlike :func:`agentless_mcp.core.gitinfo._run`, a failure here is an
    error, not a note: that module answers "what state is this repository in",
    where unknown is a legitimate answer, and this one performs the write-side
    operations where a failure means the operation did not happen.
    """
    subcommand = arguments[0] if arguments else "git"
    command = ["git", "--no-optional-locks", "-C", str(cwd), *arguments]
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            timeout=GIT_TIMEOUT_SECONDS,
            check=False,
        )
    except FileNotFoundError as exc:
        message = "git is not installed, so the patch machinery cannot run"
        raise AtlasError(message) from exc
    except subprocess.TimeoutExpired as exc:
        message = f"git {subcommand} timed out after {GIT_TIMEOUT_SECONDS}s in {cwd}"
        raise AtlasError(message) from exc
    except OSError as exc:
        message = f"git {subcommand} could not be run in {cwd}: {exc.strerror}"
        raise AtlasError(message) from exc

    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        first = detail.splitlines()[0] if detail else "no detail"
        message = f"git {subcommand} exited {completed.returncode} in {cwd}: {first}"
        raise AtlasError(message)

    return completed.stdout.decode("utf-8", errors="replace")


def _release(root: Path, path: Path) -> None:
    """Remove one scratch worktree and prune the repository's record of it."""
    try:
        run_git(root, ["worktree", "remove", "--force", str(path)])
    except AtlasError as exc:
        logger.warning("git worktree remove failed for %s: %s; removing the directory", path, exc)
        shutil.rmtree(path, ignore_errors=True)

    try:
        run_git(root, ["worktree", "prune"])
    except AtlasError as exc:
        logger.warning("git worktree prune failed in %s: %s", root, exc)

    if path.exists():
        logger.error(
            "scratch worktree %s could not be removed and is still on disk; delete it by hand",
            path,
        )
