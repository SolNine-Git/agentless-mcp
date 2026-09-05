"""Line-range history: the git argv that asks for it and the parser that reads the answer."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum

from agentless_mcp.util import bounds

# %b carries no terminator of its own, so with -z each commit is exactly four
# NUL-terminated fields and a body can hold any newline it likes.
HISTORY_FORMAT = "%H%x00%aI%x00%s%x00%b"
FIELDS_PER_COMMIT = 4

# The receipt's 5 s bound is a courtesy on every call; here the git call is the
# answer the caller asked for, and -L walks history computing a diff per commit.
HISTORY_TIMEOUT_SECONDS = 30.0
MAX_HISTORY_OUTPUT_BYTES = 2_000_000


@dataclass(frozen=True)
class CommitRecord:
    """One commit that touched the span: sha, author date, subject and body."""

    sha: str
    authored: str
    subject: str
    body: str


class HistoryFailure(Enum):
    """Why git could not answer, classified from the runner's note."""

    NO_PATH = "no_path"
    SPAN_BEYOND_HEAD = "span_beyond_head"
    TIMEOUT = "timeout"
    NO_GIT = "no_git"
    OTHER = "other"


def log_arguments(path: str, start: int, end: int, *, max_count: int) -> list[str]:
    """Build the git log argv for the commits that touched lines start..end of path."""
    bounds.at_least(start, 1, "start")
    bounds.at_least(end, start, "end")
    bounds.at_least(max_count, 1, "max_count")
    # The path rides inside the -L token, where git reads it as a literal file
    # name relative to -C, never as a pathspec; a trailing "-- <path>" is refused.
    return [
        "log",
        f"-L{start},{end}:{path}",
        "--no-patch",
        "-z",
        f"--format={HISTORY_FORMAT}",
        f"--max-count={max_count}",
        "--",
    ]


def diff_arguments(path: str) -> list[str]:
    """Build the git diff argv whose exit status says whether path differs from HEAD."""
    return ["diff", "--quiet", "HEAD", "--", f"./{path}"]


def parse_log(text: str) -> tuple[tuple[CommitRecord, ...], bool]:
    """Parse -z log output into records; the flag says a trailing partial record was dropped."""
    fields = text.split("\x00")
    if fields and fields[-1] == "":
        fields.pop()
    whole = len(fields) // FIELDS_PER_COMMIT
    records = tuple(
        CommitRecord(*fields[index * FIELDS_PER_COMMIT : (index + 1) * FIELDS_PER_COMMIT])
        for index in range(whole)
    )
    return records, len(fields) % FIELDS_PER_COMMIT != 0


def classify_failure(note: str) -> HistoryFailure:
    """Name the failure a runner note describes."""
    if "There is no path" in note:
        return HistoryFailure.NO_PATH
    if "has only" in note and "lines" in note:
        return HistoryFailure.SPAN_BEYOND_HEAD
    if "timed out" in note:
        return HistoryFailure.TIMEOUT
    if "not installed" in note or "could not be run" in note:
        return HistoryFailure.NO_GIT
    return HistoryFailure.OTHER
