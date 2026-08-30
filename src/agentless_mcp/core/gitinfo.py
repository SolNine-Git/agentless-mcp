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
system monitors and external diff drivers are disabled, and pager output is
forced through ``cat``. The prefix is public within the core so the walker and
write-side sandbox cannot drift from the receipt code.
"""

import logging
import os
import subprocess
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

# Bounded hard: the receipt is a courtesy on every call, and a courtesy that
# can hang is a bug. Five seconds is far above a healthy `git status` on a
# large repository and far below anything a caller would wait through.
GIT_TIMEOUT_SECONDS = 5.0

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
)


def subprocess_env() -> dict[str, str]:
    """Return the environment a git call this package makes may inherit.

    Every ``GIT_``-prefixed variable is removed, and nothing else is. The
    variables that break this package are the ones that redirect git away from
    the ``-C`` it was given -- ``GIT_DIR``, ``GIT_WORK_TREE``,
    ``GIT_INDEX_FILE``, ``GIT_CEILING_DIRECTORIES``, ``GIT_OBJECT_DIRECTORY``
    and their relatives -- and the whole family is stripped rather than a list
    of the ones known to hurt today, because the list is git's to extend.

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
    return {name: value for name, value in os.environ.items() if not name.startswith("GIT_")}


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


@dataclass(frozen=True)
class _Outcome:
    """One git invocation: its stdout, or the reason there is none."""

    text: str | None
    note: str


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


def _parse_dirty(outcome: _Outcome) -> _DirtyOutcome:
    """Count the porcelain lines; unknown stays unknown."""
    if outcome.text is None:
        return _DirtyOutcome(count=None, note=outcome.note)
    lines = [line for line in outcome.text.splitlines() if line.strip()]
    return _DirtyOutcome(count=len(lines), note=outcome.note)


def _run(cwd: Path, arguments: Sequence[str]) -> _Outcome:
    """Run one bounded git command; every failure becomes a note, never a raise."""
    subcommand = arguments[0] if arguments else "git"
    command = ["git", *HARDENING_PREFIX, "-C", str(cwd), *arguments]
    try:
        # Fixed argv, no shell. `cwd` is an already-resolved path and the rest
        # of the argv is a literal, so nothing from the analysed repository
        # can steer this call.
        completed = subprocess.run(
            command,
            capture_output=True,
            timeout=GIT_TIMEOUT_SECONDS,
            check=False,
            env=subprocess_env(),
        )
    except FileNotFoundError:
        return _Outcome(None, "git is not installed, so repository state is unknown")
    except subprocess.TimeoutExpired:
        return _Outcome(None, f"git {subcommand} timed out after {GIT_TIMEOUT_SECONDS}s")
    except OSError as exc:
        return _Outcome(None, f"git {subcommand} could not be run: {exc.strerror}")

    if completed.returncode != 0:
        # Taking the first line keeps the note short. It is not what makes the
        # note safe to put on a receipt row -- `application/envelope` escapes
        # it at the sink, which is where the line grammar is known. Simplify
        # this to `.strip()` if the note should carry more, and nothing
        # downstream breaks.
        detail = completed.stderr.decode("utf-8", errors="replace").strip().splitlines()
        first = detail[0] if detail else "no detail"
        return _Outcome(None, f"git {subcommand} exited {completed.returncode}: {first}")

    return _Outcome(completed.stdout.decode("utf-8", errors="replace").strip(), "")
