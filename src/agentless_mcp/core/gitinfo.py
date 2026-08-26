"""Git state for the response receipt: root, HEAD, tree OID and dirty paths.

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

import os
import subprocess
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

# Bounded hard: the receipt is a courtesy on every call, and a courtesy that
# can hang is a bug. Five seconds is far above a healthy `git status` on a
# large repository and far below anything a caller would wait through.
GIT_TIMEOUT_SECONDS = 5.0

# Short SHAs are for humans reading a receipt; eight hex digits stay unique
# well past the size of repository this tool is aimed at.
SHORT_SHA_LENGTH = 8

# Keep this prefix identical on every git argv in the package. In particular,
# ``diff.external`` and ``core.fsmonitor`` can name executables in repository
# configuration, while a pager can turn a non-interactive read into an
# unbounded process. Values are fixed here; repository content never reaches
# this tuple.
HARDENING_PREFIX: tuple[str, ...] = (
    "--no-optional-locks",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.pager=cat",
    "-c",
    "diff.external=",
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
    dirty_paths: tuple[str, ...] = ()


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
    return _parse_dirty(_run(root, ["status", "--porcelain", "-z"], strip=False)).count


def snapshot(root: Path) -> GitSnapshot:
    """Read HEAD, tree OID and dirty count in one pass, notes collected."""
    if git_root(root) is None:
        return GitSnapshot(
            head_sha=None,
            tree_oid=None,
            dirty_count=None,
            note=f"{root} is not inside a git repository: HEAD and dirty count are unknown",
        )

    head = _run(root, ["rev-parse", f"--short={SHORT_SHA_LENGTH}", "HEAD"])
    tree = _run(root, ["rev-parse", f"--short={SHORT_SHA_LENGTH}", "HEAD^{tree}"])
    status = _parse_dirty(_run(root, ["status", "--porcelain", "-z"], strip=False))

    notes = [note for note in (head.note, tree.note, status.note) if note]
    return GitSnapshot(
        head_sha=head.text,
        tree_oid=tree.text,
        dirty_count=status.count,
        dirty_paths=status.paths,
        note="; ".join(notes),
    )


@dataclass(frozen=True)
class _DirtyOutcome:
    """A porcelain status parsed into a count and its paths, or why it has none."""

    count: int | None
    paths: tuple[str, ...]
    note: str


# A porcelain entry is two status characters, a space, then the path, so a
# record shorter than this carries no path to read.
_SHORTEST_ENTRY = 4


def _parse_dirty(outcome: _Outcome) -> _DirtyOutcome:
    """Parse one porcelain status into an entry count and the paths; unknown stays unknown.

    Records are NUL-separated because NUL is the one byte a path cannot
    contain. Without ``-z`` git renders a rename as ``R  old -> new``, and a
    file genuinely named ``old -> new`` is spelled identically, so the paths
    would be a guess on exactly the tree that needs them named.

    A rename or a copy emits its destination as the entry and its source as
    the record straight after. The destination is the path that now exists, so
    the source is consumed here rather than counted as a second change.
    """
    if outcome.text is None:
        return _DirtyOutcome(count=None, paths=(), note=outcome.note)

    records = [record for record in outcome.text.split("\0") if record]
    paths: list[str] = []
    entries = 0
    index = 0
    while index < len(records):
        record = records[index]
        index += 1
        entries += 1
        if len(record) >= _SHORTEST_ENTRY:
            paths.append(record[3:])
        if "R" in record[:2] or "C" in record[:2]:
            # The source record belongs to the entry just read, not to a
            # change of its own.
            index += 1
    return _DirtyOutcome(count=entries, paths=tuple(paths), note=outcome.note)


def _run(cwd: Path, arguments: Sequence[str], *, strip: bool = True) -> _Outcome:
    """Run one bounded git command; every failure becomes a note, never a raise.

    ``strip`` is what a one-value read wants: a SHA arrives with a trailing
    newline that nothing downstream should carry. A porcelain status wants the
    opposite. Its first record opens with the two status characters, and an
    unstaged modification spells the first of them as a space, so stripping
    the output deletes half the status and shifts the path it precedes.
    """
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

    text = completed.stdout.decode("utf-8", errors="replace")
    return _Outcome(text.strip() if strip else text, "")
