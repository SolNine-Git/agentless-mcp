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

Removal happens in a ``finally`` that covers the creation as well, so a patch
that raises mid-apply and a ``worktree add`` that is killed part-way both
leave nothing behind -- a half-created worktree leaves a record git's own
``prune`` refuses to touch, which would be a permanent entry in somebody
else's repository. Cleanup failures are logged with the path rather than
raised: raising from the ``finally`` would replace whatever the caller was
already failing with, and losing the real error to report a leftover directory
is the wrong trade. The log line names the directory an operator can delete.

Every git call is a fixed argv with a timeout and no shell, and the calls that
touch the analysed repository disable the parts of git that would run *its*
code: ``core.hooksPath`` and ``core.fsmonitor`` are neutralised on every
``worktree add``, because checking HEAD out fires that repository's
``post-checkout`` hook otherwise. The residual is named rather than papered
over: a ``.gitattributes`` naming a ``filter.*.smudge`` driver still runs it
on checkout, since disabling filters would hand the tests different file
contents than the repository's own checkout has.

The test runner takes the opposite kind of input -- a command string the
*caller* supplied -- and keeps the same posture. It is split with ``shlex``
and executed as an argv, never through a shell, so a command carrying ``;`` or
``$(...)`` is one argument rather than two statements. It runs in its own
session, so the whole process group can be killed when the bound expires:
a test suite that spawns a server and hangs must leave nothing running.

The child receives an explicit environment holding only the names its platform
family allows -- ``PATH``, ``HOME``, ``LANG`` and ``TMPDIR`` on POSIX, and the
Windows equivalents on Windows -- when those names exist in the parent. A
caller may opt individual additional names in, but there is no bulk
inheritance. This keeps ambient credentials out of ordinary validation runs; it
is credential containment, not process sandboxing, because the command still
runs with the caller's user identity and can read anything that identity can
read.

The bound is hard, and a command that hits it is a **failure**. A hung test
run is the case this machinery exists to catch, and the one place where
"we did not find out" must never render as green. Hard means bounded, not
instantaneous: ending a stubborn command costs at most the SIGTERM grace plus
a short reap wait after SIGKILL, so the wall-clock worst case is the timeout
plus ``TERM_GRACE_SECONDS`` plus ``KILL_REAP_SECONDS``.

Process-group control is where the platforms genuinely differ. POSIX gets the
full guarantee: a new session, then SIGTERM and SIGKILL to the whole group, so
grandchildren die with their parent. Windows gets a documented best effort --
a new process group at spawn, then ``terminate()`` and ``kill()`` on the
leader -- and that difference is stated in ``docs/functional-assessment.md``
rather than papered over, because a caller who believes stray children are
impossible on a platform where they are not has been told something false.
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
from agentless_mcp.util.errors import OperationFailed, RepoResolutionError

logger = logging.getLogger(__name__)

# Creating a worktree copies HEAD out; on a large repository that is slower
# than a status read but nowhere near a minute. A bound that can be hit by a
# healthy repository is a flake, and no bound at all is a hang.
GIT_TIMEOUT_SECONDS = 120.0

# The bound on the cleanup calls, deliberately shorter. `_release` runs under
# `_WORKTREE_LOCK`, so a stuck `remove` or `prune` blocks every other worktree
# operation in this process for as long as it is allowed to run -- and there
# are two of them, so the creation bound would put four minutes of one hung
# cleanup in front of a `validate --jobs N` pool. Removing a worktree deletes
# files git already tracks and pruning walks a handful of records; neither
# checks anything out, so neither has the reason `worktree add` has to be slow.
# When it does expire, `_release` falls through to `shutil.rmtree` and the
# directory still goes away.
GIT_CLEANUP_TIMEOUT_SECONDS = 30.0

WORKTREE_DIR = "worktrees"

# How long a timed-out process group gets between SIGTERM and SIGKILL. Long
# enough for a test runner to flush its own summary, short enough that the
# caller's timeout stays a bound they can reason about.
TERM_GRACE_SECONDS = 5.0

# How long the runner waits to reap the leader after SIGKILL. Not a second
# grace: SIGKILL cannot be caught or ignored, so this wait exists only to
# collect the exit status rather than leave a zombie, and to keep the capture
# files from being read while a writer could still exist. A leader still
# present when it expires is stuck in the kernel, and waiting a full grace
# would not unstick it -- it would only stretch the caller's timeout.
KILL_REAP_SECONDS = 1.0

# The default cap on captured output, per stream. The tail is what is kept:
# a failing test run puts its summary at the end, and the megabyte of
# progress output before it is the part nobody reads.
DEFAULT_MAX_CAPTURE = 100_000

# How far a capture file is allowed to outgrow the cap before the runner drops
# what it holds. A cap that bounds only what is read back is not a bound at
# all: the child writes at disk speed for the whole timeout window, and
# ``--jobs N`` multiplies that by ``2N`` streams. Slack rather than a hard
# equality because trimming on every poll would throw away the tail that is
# about to be reported.
CAPTURE_SLACK = 8

# How often the wait loop looks at the capture files. Short enough that a
# runaway writer is caught within a fraction of a second's output, long enough
# that a quiet command costs a handful of wakeups per second.
CAPTURE_POLL_SECONDS = 0.25

# Options that stop git running the analysed repository's own code. A
# ``worktree add`` checks HEAD out, and a checkout fires that repository's
# ``post-checkout`` hook and consults its ``core.fsmonitor`` command -- both
# of them repository-controlled execution, which is the same hazard ``diff``
# neutralises ``diff.external`` for. ``/dev/null`` is not a directory, so no
# hook is ever found under it.
NO_REPO_CODE = ("-c", "core.hooksPath=/dev/null")

# Deliberately small. These are process-operability values rather than
# credentials, and absent values stay absent instead of being invented. Any
# additional variable must be named explicitly by the validate invocation.
#
# One list per family, because the POSIX names are not something a Windows
# child can start with. A CPython child given no ``SystemRoot`` fails during
# interpreter start-up, so a single POSIX list decided that no Python test
# suite could run on the platform the process-group code was written for.
# ``PATHEXT`` is how the loader decides what is executable, ``COMSPEC`` is what
# a runner shelling out expects to find, ``TEMP``/``TMP`` are what ``TMPDIR``
# is called there, and ``USERPROFILE`` is the home directory ``HOME`` is not.
# Upper case throughout because ``os.environ`` upper-cases its keys on Windows.
POSIX_TEST_ENV_ALLOWLIST: tuple[str, ...] = ("PATH", "HOME", "LANG", "TMPDIR")
WINDOWS_TEST_ENV_ALLOWLIST: tuple[str, ...] = (
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "COMSPEC",
    "TEMP",
    "TMP",
    "USERPROFILE",
)

# ``git worktree add`` and ``git worktree prune`` race each other: prune walks
# the repository's worktree records and can remove one that a concurrent add
# has created but not yet populated. Serialising the bookkeeping within this
# process is what lets `validate --jobs N` give every candidate its own
# worktree. The lock is held across the git call only, never across the body
# of the context manager, so nested worktrees do not deadlock.
#
# It is still held across git, so the git bounds are what bound the wait: one
# stuck call blocks the pool for its own timeout. The worst case is one
# creation plus one release -- `GIT_TIMEOUT_SECONDS` +
# 2 * `GIT_CLEANUP_TIMEOUT_SECONDS` -- which is why the cleanup pair is bounded
# separately rather than inheriting the creation's minute-scale allowance.
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

    The creation is inside the ``try``, not before it: ``git worktree add``
    writes its record into ``.git/worktrees`` before it populates the
    directory, and a call that is killed, times out or runs out of disk part
    of the way through leaves that record behind marked ``locked``, which
    ``git worktree prune`` skips forever. Releasing on the way out of a failed
    creation is what keeps the promise that nothing is left in somebody else's
    repository.
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
    # Keyed on the property the module docstring promises -- the scratch is
    # never inside the target -- rather than on any particular way of getting
    # it wrong. `cache_root` now refuses a relative XDG_CACHE_HOME, which was
    # one route in; an operator pointing XDG_CACHE_HOME at a path inside the
    # repository is another, and this catches both. Checked before mkdir, so
    # a refused location is not created on the way to being refused.
    if scratch == top or top in scratch.parents:
        message = (
            f"scratch worktrees would be created at {scratch}, inside the repository "
            f"{top} they are meant to stay out of. Point {cache.ENV_CACHE_HOME} at a "
            f"directory outside the repository."
        )
        raise RepoResolutionError(message)

    # `mode=` on the create, as `core/cache.py` does, so the directory is
    # never briefly at the umask's mode. The chmod stays for the directory an
    # earlier run already made.
    scratch.mkdir(parents=True, exist_ok=True, mode=cache.DIRECTORY_MODE)
    scratch.chmod(cache.DIRECTORY_MODE)

    path = scratch / f"wt-{uuid.uuid4().hex}"
    try:
        with _WORKTREE_LOCK:
            run_git(
                top,
                ["worktree", "add", "--detach", str(path), "HEAD"],
                config=NO_REPO_CODE,
            )
        relative = resolved.relative_to(top)
        inside = path / relative
        # The worktree holds HEAD, so a directory that exists in the caller's
        # working tree but is untracked or ignored is simply not in it. Without
        # this the yielded path does not exist, and the caller's own Popen is
        # what fails -- reporting that it could not start the interpreter,
        # which names neither the directory nor the reason. Raised inside the
        # try, so the worktree just created is still released.
        if not inside.is_dir():
            message = (
                f"{resolved} is not at HEAD of {top}, so the worktree has no {relative} to "
                "work in. The directory is untracked or ignored: commit it, or point this "
                "call at a directory the repository tracks."
            )
            raise RepoResolutionError(message)
        yield inside
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
    timeout: float,
    max_capture: int = DEFAULT_MAX_CAPTURE,
    passthrough_env: Sequence[str] = (),
) -> RunResult:
    """Run one caller-supplied command in ``cwd`` under a hard time bound.

    Never raises: every way this can go wrong is a status. A command that
    cannot be parsed or started is :data:`RunStatus.ERROR`, one that outlives
    ``timeout`` seconds is :data:`RunStatus.TIMEOUT`, and only exit zero is
    :data:`RunStatus.PASSED`. A caller reading these four values cannot
    accidentally treat "we never found out" as a pass, which is the whole
    point of the type.

    Four properties are load-bearing rather than incidental:

    * **No shell.** ``shlex.split`` produces an argv and ``Popen`` receives
      it. There is no interpretation of metacharacters at any point.
    * **Own process group.** ``start_new_session=True`` on POSIX makes the
      child a process group leader, so cleanup can signal the group and reach
      the grandchildren a test runner spawned. Killing only the direct child
      leaves the server it started holding a port. The group is signalled
      whenever this function stops with the process unreaped -- the timeout
      path and an interrupt alike -- because the same session that makes the
      group reachable is what keeps an operator's Ctrl-C from reaching it.
      Windows gets
      ``CREATE_NEW_PROCESS_GROUP``, which is the closest thing it has.
      Termination is bounded too: a command that ignores SIGTERM costs at most
      ``timeout`` + :data:`TERM_GRACE_SECONDS` + :data:`KILL_REAP_SECONDS` of
      wall clock, because the wait after SIGKILL is a short reaping guard, not
      a second grace.
    * **Output goes to files, not pipes.** A pipe nobody drains fills at
      64 KB and blocks the writer, turning a chatty passing test into a
      timeout. Temporary files decouple the two, and the tail is read back
      after the process is gone. The files are bounded while they are being
      written, not only when they are read: ``max_capture`` is a cap on what
      the command may leave on disk as well as on what is reported.
    * **Environment is allowlisted.** The child receives only the names its
      platform family allows -- :data:`POSIX_TEST_ENV_ALLOWLIST` or
      :data:`WINDOWS_TEST_ENV_ALLOWLIST` -- plus names the caller explicitly
      passes through. This contains ambient credentials; it does not sandbox
      the process or constrain what the caller's user can read from disk.
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
                env=_test_environment(passthrough_env, flavour),
                stdin=subprocess.DEVNULL,
                stdout=out,
                stderr=err,
                **_group_kwargs(flavour),
            )
        except OSError as exc:
            reason = exc.strerror or str(exc)
            elapsed = time.monotonic() - started
            return _spawn_failure(f"could not start {argv[0]!r}: {reason}", elapsed)

        try:
            timed_out = _wait_bounded(process, (out, err), timeout=timeout, capture=max_capture)
        finally:
            # Not only on the timeout branch. `start_new_session=True` puts the
            # child outside the terminal's foreground process group, so an
            # operator's Ctrl-C reaches this process and never the command it
            # started. Without this the run is abandoned and the command keeps
            # running -- holding the port, the database or the lock that the
            # next run needs. `returncode is None` is the invariant that
            # matters: the process was never reaped, whatever the reason.
            if process.returncode is None:
                _kill_group(process, flavour)

        duration = round(time.monotonic() - started, 3)
        stdout_tail = _tail(out, max_capture)
        stderr_tail = _tail(err, max_capture)

    if timed_out:
        return RunResult(RunStatus.TIMEOUT, None, duration, stdout_tail, stderr_tail)

    code = process.returncode
    status = RunStatus.PASSED if code == 0 else RunStatus.FAILED
    return RunResult(status, code, duration, stdout_tail, stderr_tail)


def _env_allowlist(flavour: str) -> tuple[str, ...]:
    """Return the parent variable names a child may inherit on ``flavour``.

    A pure function of the family rather than a module-level constant, so the
    choice is testable on either platform -- the same reason
    :func:`_group_kwargs` is one.
    """
    if flavour == platforms.WINDOWS:
        return WINDOWS_TEST_ENV_ALLOWLIST
    return POSIX_TEST_ENV_ALLOWLIST


def _test_environment(passthrough: Sequence[str], flavour: str) -> dict[str, str]:
    """Return the exact parent variables one test command may inherit."""
    names = (*_env_allowlist(flavour), *passthrough)
    return {name: os.environ[name] for name in names if name in os.environ}


def _spawn_failure(reason: str, duration: float = 0.0) -> RunResult:
    """Return the result of a command that never ran, with the reason on stderr."""
    return RunResult(RunStatus.ERROR, None, round(duration, 3), "", reason)


def _wait_bounded(
    process: "subprocess.Popen[bytes]",
    streams: Sequence[IO[bytes]],
    *,
    timeout: float,
    capture: int,
) -> bool:
    """Wait for ``process``, keeping its capture files inside the cap.

    Returns True when the time bound expired. The size bound is the other half
    of the same guarantee: a command is stopped when it outlives ``timeout``,
    and what it wrote is dropped when it outgrows ``capture`` -- otherwise the
    cap describes only the tail that is read back, while the command fills
    ``TMPDIR`` at disk speed for the whole window.
    """
    ceiling = max(capture, 1) * CAPTURE_SLACK
    holes = [0] * len(streams)
    deadline = time.monotonic() + timeout
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return True
        try:
            process.wait(timeout=min(CAPTURE_POLL_SECONDS, remaining))
        except subprocess.TimeoutExpired:
            for index, handle in enumerate(streams):
                holes[index] = _trim(handle, ceiling, holes[index])
        else:
            return False


def _trim(handle: IO[bytes], ceiling: int, hole: int) -> int:
    """Drop what a capture file holds once its live bytes outgrow ``ceiling``.

    The child keeps its own file offset, so what it writes next lands past the
    end of the emptied file and everything between is a hole the filesystem
    never stores. Only the tail is ever reported, so the discarded bytes were
    never going to be read -- and :func:`_tail` drops the leading NUL bytes of
    the hole when its window reaches back into one.

    ``hole`` is the file size at the previous trim -- the offset where live
    bytes begin -- and the returned value carries it to the next poll. The
    comparison must subtract it, because ``st_size`` still counts the hole:
    judged on raw size, a once-trimmed file stays over the ceiling forever and
    every later poll truncates it again, so a poll tick landing between the
    child's final write and its observed exit silently discards the very tail
    the runner is about to report.
    """
    descriptor = handle.fileno()
    size = os.fstat(descriptor).st_size
    if size - hole <= ceiling:
        return hole
    os.ftruncate(descriptor, 0)
    return size


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

    The wait after SIGKILL is a reaping guard, not a second grace -- nothing
    can ignore SIGKILL, so :data:`KILL_REAP_SECONDS` is all it gets. That keeps
    cleanup bounded: a command that ignores SIGTERM costs at most
    :data:`TERM_GRACE_SECONDS` + :data:`KILL_REAP_SECONDS` beyond its timeout.

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
        process.wait(timeout=KILL_REAP_SECONDS)
    except subprocess.TimeoutExpired:
        logger.exception(
            "process group %d is still present after SIGKILL; it may be stuck in the kernel",
            group,
        )


def _kill_leader(process: "subprocess.Popen[bytes]") -> None:
    """End the timed-out command's leader process, politely then not.

    The Windows path. Anything the command spawned survives it, which is why
    ``docs/functional-assessment.md`` says the timeout guarantee there is best
    effort: without a job object there is nothing to signal a whole tree with.
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
        process.wait(timeout=KILL_REAP_SECONDS)
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

    Leading NUL bytes are dropped because the window can reach back into the
    hole :func:`_trim` leaves behind: the file's length still counts the bytes
    a runaway command wrote before the trim, and reading zeros back as output
    would put a wall of NULs in front of the summary.
    """
    end = handle.seek(0, os.SEEK_END)
    if end == 0:
        return ""

    start = max(0, end - max(limit, 0))
    handle.seek(start)
    text = handle.read().lstrip(b"\x00").decode("utf-8", errors="replace")
    if start == 0:
        return text
    return f"[... {start} earlier bytes dropped ...]\n{text}"


def run_git(
    cwd: Path,
    arguments: Sequence[str],
    *,
    config: Sequence[str] = (),
    timeout: float = GIT_TIMEOUT_SECONDS,
) -> str:
    """Run one bounded git command, raising on anything but success.

    Unlike :func:`agentless_mcp.core.gitinfo._run`, a failure here is an
    error, not a note: that module answers "what state is this repository in",
    where unknown is a legitimate answer, and this one performs the write-side
    operations where a failure means the operation did not happen.

    ``config`` carries ``-c key=value`` pairs that go in front of the
    subcommand, which is how a caller overrides what the analysed repository's
    own configuration would otherwise decide -- see :data:`NO_REPO_CODE`.

    ``timeout`` defaults to the creation bound. A caller holding a lock across
    this call should pass a smaller one, because the bound it chooses is how
    long every other holder of that lock waits.
    """
    subcommand = arguments[0] if arguments else "git"
    command = ["git", *gitinfo.HARDENING_PREFIX, *config, "-C", str(cwd), *arguments]
    try:
        completed = subprocess.run(
            command,
            capture_output=True,
            timeout=timeout,
            check=False,
            # The write-side calls have most to lose from an ambient GIT_DIR:
            # `worktree add` against a redirected repository would create the
            # checkout somewhere the caller never named.
            env=gitinfo.subprocess_env(),
        )
    except FileNotFoundError as exc:
        message = "git is not installed, so the patch machinery cannot run"
        raise OperationFailed(message) from exc
    except subprocess.TimeoutExpired as exc:
        message = f"git {subcommand} timed out after {timeout}s in {cwd}"
        raise OperationFailed(message) from exc
    except OSError as exc:
        message = f"git {subcommand} could not be run in {cwd}: {exc.strerror}"
        raise OperationFailed(message) from exc

    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", errors="replace").strip()
        first = detail.splitlines()[0] if detail else "no detail"
        message = f"git {subcommand} exited {completed.returncode} in {cwd}: {first}"
        raise OperationFailed(message)

    return completed.stdout.decode("utf-8", errors="replace")


def _release(root: Path, path: Path) -> None:
    """Remove one scratch worktree and prune the repository's record of it.

    ``--force`` twice rather than once: a creation that was killed part-way
    leaves its record marked ``locked``, and git refuses to remove a locked
    worktree on a single force. That record is the one ``prune`` skips, so
    without the second force the failed-creation path cannot clean up after
    itself. The worktree is this module's own scratch directory either way,
    so there is nothing a force can destroy that was not already disposable.
    """
    try:
        run_git(
            root,
            ["worktree", "remove", "--force", "--force", str(path)],
            timeout=GIT_CLEANUP_TIMEOUT_SECONDS,
        )
    except OperationFailed as exc:
        logger.warning("git worktree remove failed for %s: %s; removing the directory", path, exc)
        shutil.rmtree(path, ignore_errors=True)

    try:
        run_git(root, ["worktree", "prune"], timeout=GIT_CLEANUP_TIMEOUT_SECONDS)
    except OperationFailed as exc:
        logger.warning("git worktree prune failed in %s: %s", root, exc)

    if path.exists():
        logger.error(
            "scratch worktree %s could not be removed and is still on disk; delete it by hand",
            path,
        )
