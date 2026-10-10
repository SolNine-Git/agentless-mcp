"""Files the working tree changed since a commit: the git argv that lists them, and the parser."""

import re
from collections.abc import Iterator
from dataclasses import dataclass
from enum import Enum

from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import OperationFailed

# Rename detection compares every added file with every deleted one, and the
# untracked listing walks the whole tree, so both get more than a receipt call.
LISTING_TIMEOUT_SECONDS = 30.0

# A listing cut short would report a function moved from an unlisted file as new.
MAX_LISTING_BYTES = 8_000_000

# rev-parse prints one object name, so this bounds a git that prints something else.
MAX_COMMIT_OUTPUT_BYTES = 4_096

# Git's own rule: a NUL byte in the first 8000 bytes makes a file binary.
BINARY_PROBE_BYTES = 8_000

UNTRACKED = "?"
COMPARED_STATUSES = frozenset({"A", "C", "D", "M", "R", "T", UNTRACKED})

UNTRACKED_ARGUMENTS = ("ls-files", "--others", "--exclude-standard", "-z")

_SHA_PATTERN = re.compile(r"[0-9a-f]{40}(?:[0-9a-f]{24})?")
_META_FIELDS = 5
_TWO_PATH_STATUSES = frozenset({"R", "C"})


class Side(Enum):
    """What one side of a change is at its path."""

    ABSENT = "absent"
    FILE = "file"
    LINK = "link"
    GITLINK = "gitlink"


_SIDES = {
    "000000": Side.ABSENT,
    "100644": Side.FILE,
    "100755": Side.FILE,
    "120000": Side.LINK,
    "160000": Side.GITLINK,
}


@dataclass(frozen=True)
class Change:
    """One path that differs from the commit: git's status, the old path, and both sides."""

    status: str
    path: str
    old_path: str
    old: Side
    new: Side


def revision_arguments(revision: str) -> list[str]:
    """Build the rev-parse argv that resolves a caller's revision to one full commit name."""
    return ["rev-parse", "--verify", "--end-of-options", f"{revision}^{{commit}}"]


def changes_arguments(commit: str) -> list[str]:
    """Build the diff argv that lists every path the working tree changed since ``commit``."""
    # --raw carries both modes, which name a symlink or a submodule. --relative
    # keeps every path relative to -C when the root is below the git top level.
    return ["diff", "--raw", "-z", "-M", "--relative", commit, "--"]


def parse_commit(text: str) -> str:
    """Return the one full object name rev-parse printed, or raise OperationFailed."""
    name = text.strip()
    if not _SHA_PATTERN.fullmatch(name):
        detail = "rev-parse did not print one full object name"
        raise OperationFailed(MESSAGES.lint_revision_output_malformed.format(detail=detail))
    return name


def parse_changes(raw: bytes) -> tuple[Change, ...]:
    """Parse ``git diff --raw -z`` output, or raise OperationFailed on a record it cannot read."""
    fields = _fields(raw)
    changes: list[Change] = []
    for meta in fields:
        old_mode, new_mode, status = _meta(meta)
        paths = [_next_path(fields) for _ in range(2 if status in _TWO_PATH_STATUSES else 1)]
        old = Side.ABSENT if status == "C" else _side(old_mode)
        changes.append(Change(status, paths[-1], paths[0], old, _side(new_mode)))
    return tuple(changes)


def parse_untracked(raw: bytes) -> tuple[Change, ...]:
    """Parse ``git ls-files --others -z`` output; a name ending in "/" is a nested repository."""
    changes = []
    for field in _fields(raw):
        name = _decoded(field)
        nested = name.endswith("/")
        path = name.rstrip("/") if nested else name
        new = Side.GITLINK if nested else Side.FILE
        changes.append(Change(UNTRACKED, path, path, Side.ABSENT, new))
    return tuple(changes)


def looks_binary(data: bytes) -> bool:
    """Return whether ``data`` is binary by git's rule, a NUL byte near its start."""
    return b"\0" in data[:BINARY_PROBE_BYTES]


def _fields(raw: bytes) -> Iterator[bytes]:
    fields = raw.split(b"\0")
    if fields and fields[-1] == b"":
        fields.pop()
    return iter(fields)


def _meta(field: bytes) -> tuple[str, str, str]:
    text = _decoded(field)
    parts = text[1:].split(" ") if text.startswith(":") else []
    if len(parts) != _META_FIELDS or not parts[4]:
        detail = "a diff record does not open with ':<mode> <mode> <sha> <sha> <status>'"
        raise OperationFailed(MESSAGES.lint_revision_output_malformed.format(detail=detail))
    return parts[0], parts[1], parts[4][0]


def _next_path(fields: Iterator[bytes]) -> str:
    field = next(fields, None)
    if field is None:
        detail = "a diff record ends before its path"
        raise OperationFailed(MESSAGES.lint_revision_output_malformed.format(detail=detail))
    return _decoded(field)


def _side(mode: str) -> Side:
    side = _SIDES.get(mode)
    if side is None:
        detail = f"a diff record carries the mode {mode[:12]!r}, which names no file type"
        raise OperationFailed(MESSAGES.lint_revision_output_malformed.format(detail=detail))
    return side


def _decoded(field: bytes) -> str:
    # A name that is not UTF-8 decodes with replacement characters, so reading it
    # fails and becomes a row rather than naming some other file.
    return field.decode("utf-8", errors="replace")
