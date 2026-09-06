"""Git state for the response receipt: root, HEAD, tree OID and dirty count.

Every answer this package produces carries the state of the repository it was
computed from, so an agent can tell a stale answer from a fresh one and a
wrong-repository answer from a right one. That state comes from git, which is
an out-of-process call like any other: it gets a timeout, it never runs
through a shell, and nothing from repository content reaches its argv.

Unknown is a value here, not an error. A directory that is not a git
repository, a repository with no commits, a machine without git installed and
a call that timed out all produce ``None`` plus a note saying which of those
happened -- the receipt then reads ``head: nogit`` or ``dirty: unknown``
rather than claiming a clean tree nobody checked.

``--no-optional-locks`` is passed on every invocation: reading the state of a
repository must not take a lock or refresh an index in a tree we are only ever
allowed to read.

Repository-local configuration is untrusted input. Every git invocation in
the package therefore carries the same fixed configuration prefix: file
system monitors, external diff drivers and commit-signature verification are
disabled, and pager output is forced through ``cat``. The prefix cannot drift
between callers because this module owns the only place the package spawns
git: the walker and the write-side sandbox run their own argv through
:func:`run_bounded`, so the prefix, the scrubbed environment, the
deadline and the output cap reach every git call the package makes.
"""

import logging
import os
import selectors
import subprocess
import sys
import time
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import IO

from agentless_mcp.util import platforms

# Bounded hard: the receipt is a courtesy on every call, and a courtesy that
# can hang is a bug. Five seconds is far above a healthy `git status` on a
# large repository and far below anything a caller would wait through.
GIT_TIMEOUT_SECONDS = 5.0

# The receipt reads a short SHA, a porcelain status and a churn log, so this is
# far above any healthy answer and only bounds the pathological one.
MAX_RECEIPT_OUTPUT_BYTES = 8_000_000

# A note quotes stderr's first line, so the rest is only ever read past. The
# pipe is drained past this point anyway: a child blocked on it never exits.
MAX_STDERR_BYTES = 65_536

logger = logging.getLogger(__name__)

# Short SHAs are for humans reading a receipt; eight hex digits stay unique
# well past the size of repository this tool is aimed at.
SHORT_SHA_LENGTH = 8

# Keep this prefix identical on every git argv in the package. In particular,
# ``diff.external`` and ``core.fsmonitor`` can name executables in repository
# configuration, while a pager can turn a non-interactive read into an
# unbounded process. Values are fixed here; repository content never reaches
# this tuple.
#
# The last entry keeps git's *output* literal rather than its execution safe.
# ``core.quotePath`` defaults to true, which prints any path with a byte over
# 0x7f as a quoted, octal-escaped C string -- so a parser matching printed
# names against the spellings it asked about silently misses every non-ASCII
# filename, and :func:`commit_churn` reported such a file as measured-quiet
# (``0c``) rather than unknown. Not ``--literal-pathspecs``: that flag also
# stops an absolute path from resolving as a pathspec, which
# ``treewalk._git_ignores`` depends on; pathspec-magic defense is scoped to
# the one caller that passes repository-named paths (see ``commit_churn``).
HARDENING_PREFIX: tuple[str, ...] = (
    "--no-optional-locks",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.pager=cat",
    "-c",
    "diff.external=",
    "-c",
    "core.quotePath=false",
    # `log.showSignature` makes every `git log` verify commit signatures through
    # `gpg.program`, and both keys are repository-local: a read becomes an exec.
    "-c",
    "log.showSignature=false",
)

# The two ``GIT_`` names that move where config is read from without moving
# which repository is read. See :func:`subprocess_env`.
GIT_CONFIG_KEPT: frozenset[str] = frozenset({"GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM"})


def subprocess_env() -> dict[str, str]:
    """Return the environment a git call this package makes may inherit.

    Every ``GIT_``-prefixed variable is removed except :data:`GIT_CONFIG_KEPT`,
    and nothing else is. The variables that break this package are the ones
    that redirect git away from the ``-C`` it was given -- ``GIT_DIR``,
    ``GIT_WORK_TREE``, ``GIT_INDEX_FILE``, ``GIT_CEILING_DIRECTORIES``,
    ``GIT_OBJECT_DIRECTORY`` and their relatives -- and the whole family is
    stripped rather than a list of the ones known to hurt today, because the
    list is git's to extend.

    The two exceptions move where config is read from, not which repository is
    read, so a caller's pinned git configuration reaches these calls.

    Reproduced before this existed: with ``GIT_DIR`` pointing at an unrelated
    repository, the receipt for the analysed repository carried the *other*
    repository's HEAD, and with ``GIT_INDEX_FILE`` pointing at a path that
    does not exist, a clean tree reported two dirty files. The receipt is how
    an agent knows which commit an answer describes, so a wrong one there is
    not a cosmetic error.

    Everything else is kept. ``PATH`` finds the binary and ``HOME`` finds the
    global config, and this package already states the configuration it needs
    on the argv -- :data:`HARDENING_PREFIX` -- rather than through the
    environment.
    """
    return {
        name: value
        for name, value in os.environ.items()
        if not name.startswith("GIT_") or name in GIT_CONFIG_KEPT
    }


@dataclass(frozen=True)
class GitSnapshot:
    """The repository state one call observed, or what stopped it observing.

    ``note`` is empty when everything was answered. It is the degradation
    channel: a missing git binary, a repository without commits or a timed-out
    status lands there instead of raising or silently reading as clean.
    """

    head_sha: str | None
    tree_oid: str | None
    dirty_count: int | None
    note: str


# The payload stays bytes because ``-z`` output carries filesystem names that
# are not always UTF-8, and a lossy decode there names files that do not exist.
@dataclass(frozen=True)
class GitOutcome:
    """One bounded git invocation: its stdout, or the reason there is none, and how it ended."""

    stdout: bytes | None
    note: str
    truncated: bool = False
    returncode: int | None = None

    @property
    def text(self) -> str | None:
        """The stdout decoded lossily, or None when there is none."""
        return None if self.stdout is None else self.stdout.decode("utf-8", errors="replace")


def git_root(path: Path) -> Path | None:
    """Return the enclosing repository's top level, or None outside one."""
    start = path if path.is_dir() else path.parent
    outcome = _run(start, ["rev-parse", "--show-toplevel"])
    if outcome.text is None:
        return None
    return Path(outcome.text).resolve()


def head_sha(root: Path) -> str | None:
    """Return the short HEAD SHA, or None when there is no commit to name."""
    return _run(root, ["rev-parse", f"--short={SHORT_SHA_LENGTH}", "HEAD"]).text


def tree_oid(root: Path) -> str | None:
    """Return the short tree OID of HEAD, the cache generation identifier."""
    return _run(root, ["rev-parse", f"--short={SHORT_SHA_LENGTH}", "HEAD^{tree}"]).text


def dirty_count(root: Path) -> int | None:
    """Return the number of modified or untracked paths; None when unknown."""
    return _parse_dirty(_run(root, ["status", "--porcelain"])).count


# The window a map header's churn suffix is counted over. One constant so the
# renderer's "90d" and the git call that produced the number cannot drift.
CHURN_WINDOW_DAYS = 90


@dataclass(frozen=True)
class ChurnFact:
    """One path's commit activity inside the churn window.

    ``last_commit_ts`` is the newest in-window commit's unix timestamp, and
    None when the window holds no commit for the path -- known-quiet, which a
    consumer must keep distinct from the whole lookup failing (that is
    :func:`commit_churn` returning None).
    """

    commits: int
    last_commit_ts: int | None


@dataclass(frozen=True)
class ChurnSource:
    """A lazy churn lookup bound to one served root.

    Built by ``resolve_repo`` only when git answered for the root, and carried
    on the context so a view that never asks pays nothing. The git call
    happens per ``for_paths`` invocation, scoped to the paths the view ranked.
    """

    root: Path

    def for_paths(self, paths: Sequence[str]) -> dict[str, ChurnFact] | None:
        """Answer churn for ``paths``, or None when git cannot say."""
        return commit_churn(self.root, paths)


def commit_churn(
    root: Path, paths: Sequence[str], *, window_days: int = CHURN_WINDOW_DAYS
) -> dict[str, ChurnFact] | None:
    """Count each path's commits inside the window, newest timestamp kept.

    One bounded ``git log`` for the whole batch. None means git could not
    answer -- not a repository, timed out, not installed -- which the caller
    must keep distinct from a dict of zeros: zeros claim quiet history, None
    claims nothing.

    The pretty format prefixes each commit's timestamp with a NUL byte
    because ``--name-only`` prints bare filenames on their own lines and a
    filename can be all digits; no filename can contain NUL. ``--relative``
    keeps printed names relative to ``root`` when it sits inside a larger
    repository, matching the spelling the caller's paths use.
    """
    if not paths:
        return {}
    outcome = _run(
        root,
        [
            "log",
            f"--since={window_days}.days",
            "--pretty=%x00%ct",
            "--name-only",
            "--relative",
            "--",
            # "./" defeats pathspec-magic detection, which triggers only on a
            # leading ":": without it a tracked file named ":!x.py" parses as
            # an exclude pattern -- it never matches itself and suppresses
            # matches for the rest of the batch. Git normalizes the prefix
            # away, so printed names still match ``paths`` exactly.
            *(f"./{path}" for path in paths),
        ],
    )
    if outcome.text is None:
        # The caller renders None as a bare header -- the documented "git
        # could not answer" -- so the note _run produced is the only place
        # the reason survives. Debug, not warning: a root outside git takes
        # this path on every map, and that is a state, not a fault.
        logger.debug("churn for %s went unanswered: %s", root, outcome.note)
        return None

    counts = dict.fromkeys(paths, 0)
    newest: dict[str, int] = {}
    current_ts: int | None = None
    for raw in outcome.text.splitlines():
        line = raw.strip()
        if not line:
            continue
        if line.startswith("\x00"):
            stamp = line[1:]
            current_ts = int(stamp) if stamp.isdigit() else None
            continue
        if line in counts and current_ts is not None:
            counts[line] += 1
            # The log is newest-first, so the first sighting is the newest.
            newest.setdefault(line, current_ts)
    return {
        path: ChurnFact(commits=counts[path], last_commit_ts=newest.get(path)) for path in paths
    }


def snapshot(root: Path) -> GitSnapshot:
    """Read HEAD, tree OID and dirty count in one pass, notes collected.

    Git answers for the repository that encloses ``root``, which is not always
    ``root`` itself. A directory analysed inside a larger repository -- a
    vendored tree, a snapshot never given a git of its own -- is served that
    repository's HEAD and that repository's dirty count, and a reader with only
    the receipt cannot tell. The note says whose state it is, so the answer is
    qualified rather than quietly wrong.
    """
    enclosing = git_root(root)
    if enclosing is None:
        return GitSnapshot(
            head_sha=None,
            tree_oid=None,
            dirty_count=None,
            note=f"{root} is not inside a git repository: HEAD and dirty count are unknown",
        )

    head = _run(root, ["rev-parse", f"--short={SHORT_SHA_LENGTH}", "HEAD"])
    tree = _run(root, ["rev-parse", f"--short={SHORT_SHA_LENGTH}", "HEAD^{tree}"])
    status = _parse_dirty(_run(root, ["status", "--porcelain"]))

    borrowed = enclosing != root.resolve()
    enclosing_note = (
        f"{root} is not the top of its git repository: HEAD and dirty count describe {enclosing}"
    )
    notes = [enclosing_note] if borrowed else []
    notes += [note for note in (head.note, tree.note, status.note) if note]
    return GitSnapshot(
        head_sha=head.text,
        tree_oid=tree.text,
        dirty_count=status.count,
        note="; ".join(notes),
    )


@dataclass(frozen=True)
class _DirtyOutcome:
    """A porcelain status parsed into a count, or the reason it has none."""

    count: int | None
    note: str


def _parse_dirty(outcome: GitOutcome) -> _DirtyOutcome:
    """Count the porcelain lines; unknown stays unknown."""
    if outcome.text is None:
        return _DirtyOutcome(count=None, note=outcome.note)
    lines = [line for line in outcome.text.splitlines() if line.strip()]
    return _DirtyOutcome(count=len(lines), note=outcome.note)


def _run(cwd: Path, arguments: Sequence[str]) -> GitOutcome:
    """Run one receipt-bounded git command; every failure becomes a note, never a raise."""
    outcome = run_bounded(
        cwd, arguments, timeout=GIT_TIMEOUT_SECONDS, max_output_bytes=MAX_RECEIPT_OUTPUT_BYTES
    )
    if outcome.truncated:
        # Unknown, not partial: half a porcelain status would undercount a
        # dirty tree, and a receipt row that reads low is worse than one unread.
        subcommand = arguments[0] if arguments else "git"
        return GitOutcome(
            None,
            f"git {subcommand} printed more than {MAX_RECEIPT_OUTPUT_BYTES} bytes",
            returncode=outcome.returncode,
        )
    if outcome.stdout is None:
        return outcome
    return GitOutcome(outcome.stdout.strip(), outcome.note, returncode=outcome.returncode)


def run_bounded(
    cwd: Path,
    arguments: Sequence[str],
    *,
    timeout: float,
    max_output_bytes: int,
    config: Sequence[str] = (),
) -> GitOutcome:
    """Run one git command under a deadline and an output cap; every failure becomes a note."""
    subcommand = arguments[0] if arguments else "git"
    command = ["git", *HARDENING_PREFIX, *config, "-C", str(cwd), *arguments]
    # LC_ALL=C keeps git's failure text in the spelling the callers classify.
    env = {**subprocess_env(), "LC_ALL": "C"}
    try:
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    except OSError as exc:
        if isinstance(exc, FileNotFoundError):
            return GitOutcome(None, "git is not installed, so repository state is unknown")
        return GitOutcome(None, f"git {subcommand} could not be run: {exc.strerror}")
    deadline = time.monotonic() + timeout
    with process:
        if process.stdout is None or process.stderr is None:
            process.kill()
            return GitOutcome(None, f"git {subcommand} could not be run: no pipes")
        # select() takes sockets and not pipes on Windows, so the incremental
        # drain is POSIX-only and Windows caps what communicate() already read.
        if platforms.family(sys.platform) == platforms.WINDOWS:
            stdout, stderr, truncated, timed_out = _drain_communicate(
                process, deadline=deadline, max_output_bytes=max_output_bytes
            )
        else:
            stdout, stderr, truncated, timed_out = _drain_selectors(
                process.stdout, process.stderr, deadline=deadline, max_output_bytes=max_output_bytes
            )
        if truncated or timed_out:
            process.kill()
        elif not _reaped_by(process, deadline):
            # Both pipes are at EOF, so the child is on its way out; bounding
            # the reap anyway leaves no unbounded wait in this runner.
            process.kill()
            timed_out = True
        process.wait()
    if timed_out:
        return GitOutcome(None, f"git {subcommand} timed out after {timeout}s")
    return _bounded_outcome(subcommand, process.returncode, stdout, stderr, truncated=truncated)


def _bounded_outcome(
    subcommand: str, returncode: int, stdout: bytes, stderr: bytes, *, truncated: bool
) -> GitOutcome:
    if truncated:
        return GitOutcome(stdout, "", truncated=True, returncode=returncode)
    if returncode != 0:
        # The first line keeps the note short; it is not what makes it safe on a
        # receipt row -- `application/envelope` escapes it where the grammar is known.
        detail = stderr.decode("utf-8", errors="replace").strip().splitlines()
        first = detail[0] if detail else "no detail"
        return GitOutcome(
            None, f"git {subcommand} exited {returncode}: {first}", returncode=returncode
        )
    return GitOutcome(stdout, "", returncode=0)


def _reaped_by(process: subprocess.Popen[bytes], deadline: float) -> bool:
    try:
        process.wait(timeout=max(0.0, deadline - time.monotonic()))
    except subprocess.TimeoutExpired:
        return False
    return True


def _drain_selectors(
    stdout: IO[bytes], stderr: IO[bytes], *, deadline: float, max_output_bytes: int
) -> tuple[bytes, bytes, bool, bool]:
    out_fd = stdout.fileno()
    buffers: dict[int, bytearray] = {out_fd: bytearray(), stderr.fileno(): bytearray()}
    truncated = timed_out = False
    with selectors.DefaultSelector() as selector:
        selector.register(stdout, selectors.EVENT_READ)
        selector.register(stderr, selectors.EVENT_READ)
        while selector.get_map() and not truncated:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                timed_out = True
                break
            for key, _events in selector.select(remaining):
                chunk = os.read(key.fd, 65536)
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                buffer = buffers[key.fd]
                if key.fd != out_fd:
                    buffer += chunk[: max(0, MAX_STDERR_BYTES - len(buffer))]
                    continue
                if len(buffer) + len(chunk) > max_output_bytes:
                    buffer += chunk[: max_output_bytes - len(buffer)]
                    truncated = True
                    break
                buffer += chunk
    return bytes(buffers[out_fd]), bytes(buffers[stderr.fileno()]), truncated, timed_out


def _drain_communicate(
    process: subprocess.Popen[bytes], *, deadline: float, max_output_bytes: int
) -> tuple[bytes, bytes, bool, bool]:
    try:
        stdout, stderr = process.communicate(timeout=max(0.0, deadline - time.monotonic()))
    except subprocess.TimeoutExpired:
        return b"", b"", False, True
    return (
        stdout[:max_output_bytes],
        stderr[:MAX_STDERR_BYTES],
        len(stdout) > max_output_bytes,
        False,
    )
