"""The SEARCH/REPLACE edit format: parsing blocks and applying them.

This is the write side's parser and applier, ported from Agentless
(``agentless/util/postprocess_data.py``) with the defects that port exposed
fixed rather than carried across. The format is the one the Agentless repair
prompt specifies, and an agent writing edits for this tool writes exactly it:

    ### path/to/file.py
    <<<<<<< SEARCH
    the exact lines to find
    =======
    the lines to put in their place
    >>>>>>> REPLACE

The path header may be fenced (```` ```python ```` and a closing ```` ``` ````
around the block, any language label) or bare, may be written ``### path`` or
``path``, and is inherited by a following block that omits it. A header line that is
not shaped like a path is prose and is skipped, so a block introduced by a
sentence names no file rather than naming the sentence. Filenames are
taken verbatim after stripping: the original wrapped them in quotes and used
``eval()`` to unwrap them at apply time, which is remote code execution
reachable from model output. Neither the quoting nor the ``eval`` is here.

Three behavioural fixes over the original, each pinned by a test:

* **Empty intervals search the whole file.** The original referenced
  ``original`` and ``replace`` before assignment when a file had no location
  intervals, so that path raised ``NameError`` instead of applying anything.
  No intervals now means the whole file is in scope, which is what the code
  was reaching for.
* **Every non-applied block reports why.** The original printed
  ``"not replaced"`` to stdout and returned the unchanged content, so a caller
  could not tell a no-op patch from an applied one. Each edit here returns a
  structured outcome: applied, not found, ambiguous with a match count, or
  outside the intervals it was scoped to.
* **Ambiguity is refused, not guessed.** ``str.replace`` in the original
  rewrote *every* occurrence of the search text. Search text matching more
  than once is now reported with the count and applied nowhere, because which
  one the author meant is not knowable from the block.

Two structural rules are preserved from the original because they are what
make the format work at all. Matching is **whole-line**: both the haystack and
the needle are wrapped in newline sentinels, so a search block can never match
a fragment of a longer line. And ``...`` **elisions** are honoured: a search
block of just ``...`` is anchored to a unique unindented line in scope, and a
leading ``...`` line on either side is dropped. A block that elides *both*
sides describes no change and is refused at parse time.

Nothing here touches the filesystem. Paths are dictionary keys; reading and
writing files, and deciding which paths a caller may name at all, belong to
:mod:`agentless_mcp.application.patch_service`.
"""

from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from enum import Enum
from typing import Any

SEARCH_MARKER = "<<<<<<< SEARCH"
DIVIDER = "======="
REPLACE_MARKER = ">>>>>>> REPLACE"
FENCE = "```"

# The elision marker and the prefix form that is stripped from either side of
# a block. `...` alone means "find me an anchor"; `...\n` at the start of a
# side means "the rest of this side is what matters".
ELISION = "..."
ELISION_PREFIX = ELISION + "\n"

# What a header line may look like and still be read as a filename rather than
# as prose: at most this many whitespace-separated words, and an extension of
# at most this many characters on the last path component.
_MAX_PATH_WORDS = 3
_MAX_EXTENSION = 12


@dataclass(frozen=True)
class Edit:
    """One SEARCH/REPLACE block: which file, what to find, what to write.

    ``index`` is the block's position in the text it was parsed from, so an
    error message can name the block a human can go and look at. It is not an
    identity: two edits with the same index are two edits.
    """

    index: int
    path: str
    search: str
    replace: str

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this edit."""
        return {
            "index": self.index,
            "path": self.path,
            "search": self.search,
            "replace": self.replace,
        }


@dataclass(frozen=True)
class BlockError:
    """A block that could not be read as an edit, and why.

    ``path`` is whatever file the block named or inherited, which is often
    known even when the block itself is malformed; ``None`` means the parser
    never saw a path to attribute it to.
    """

    index: int
    path: str | None
    reason: str

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this error."""
        return {"index": self.index, "path": self.path, "reason": self.reason}


@dataclass(frozen=True)
class ParseResult:
    """Everything one patch text yielded: the edits, and the blocks that failed.

    Both halves are always present. A parse that returns edits and drops the
    malformed blocks silently is the failure mode this type exists to prevent:
    a truncated patch would otherwise apply its first half and report success.
    """

    edits: tuple[Edit, ...]
    errors: tuple[BlockError, ...]

    @property
    def ok(self) -> bool:
        """True when every block in the text parsed as an edit."""
        return not self.errors

    def as_dict(self) -> dict[str, Any]:
        """Return the ``edits.json`` document this result serialises to."""
        return {
            "edits": [edit.as_dict() for edit in self.edits],
            "errors": [error.as_dict() for error in self.errors],
        }


class EditStatus(str, Enum):
    """What became of one edit.

    ``str, Enum`` rather than ``StrEnum`` for the 3.10 floor, matching
    :class:`agentless_mcp.core.symbols.SymbolKind`.
    """

    APPLIED = "applied"
    NOT_FOUND = "not_found"
    AMBIGUOUS = "ambiguous"
    OUTSIDE_INTERVALS = "outside_intervals"
    NO_SUCH_FILE = "no_such_file"
    UNREADABLE = "unreadable"
    NO_ANCHOR = "no_anchor"

    def __str__(self) -> str:
        """Return the member value, matching ``enum.StrEnum`` semantics."""
        return self.value


@dataclass(frozen=True)
class EditOutcome:
    """One edit's result, with the count behind an ambiguity refusal."""

    edit: Edit
    status: EditStatus
    reason: str
    matches: int = 0

    @property
    def applied(self) -> bool:
        """True when this edit changed the file."""
        return self.status is EditStatus.APPLIED

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this outcome."""
        return {
            "index": self.edit.index,
            "path": self.edit.path,
            "status": self.status.value,
            "reason": self.reason,
            "matches": self.matches,
        }


@dataclass(frozen=True)
class ApplyResult:
    """The outcome of applying a set of edits to a set of file contents.

    ``new_contents`` holds only the files an edit actually changed, so a
    caller can write exactly those back. A file every edit failed against does
    not appear, which is what keeps "write the result" from rewriting a file
    with its own contents and producing an empty diff nobody asked for.
    """

    outcomes: tuple[EditOutcome, ...]
    new_contents: dict[str, str]

    @property
    def ok(self) -> bool:
        """True when every edit applied."""
        return all(outcome.applied for outcome in self.outcomes)

    @property
    def failures(self) -> tuple[EditOutcome, ...]:
        """The edits that did not apply, in input order."""
        return tuple(outcome for outcome in self.outcomes if not outcome.applied)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this result, contents excluded."""
        return {
            "applied": sum(1 for outcome in self.outcomes if outcome.applied),
            "total": len(self.outcomes),
            "changed_files": sorted(self.new_contents),
            "outcomes": [outcome.as_dict() for outcome in self.outcomes],
        }


# ---------------------------------------------------------------------------
# Parsing
# ---------------------------------------------------------------------------


def parse_blocks(text: str) -> ParseResult:
    """Parse ``text`` into edits, reporting every block that is not one.

    Blocks are delimited by ``>>>>>>> REPLACE``; everything before a block's
    ``<<<<<<< SEARCH`` is its header, and the last non-blank, non-fence line
    of that header is the file path. A block with no path of its own inherits
    the last one seen, which is how several consecutive edits to one file are
    written.
    """
    segments = text.split(REPLACE_MARKER)
    edits: list[Edit] = []
    errors: list[BlockError] = []
    current_path: str | None = None

    for index, segment in enumerate(segments[:-1]):
        parts = segment.split(SEARCH_MARKER)
        named = _filename_in(parts[0])
        if named is not None:
            current_path = named

        markers = len(parts) - 1
        if markers != 1:
            errors.append(BlockError(index, current_path, _marker_reason(markers)))
            continue

        if current_path is None:
            errors.append(
                BlockError(
                    index,
                    None,
                    "block names no file: put the path on its own line above <<<<<<< SEARCH",
                )
            )
            continue

        body = _split_body(parts[1])
        if body.reason:
            errors.append(BlockError(index, current_path, body.reason))
            continue

        edits.append(Edit(index=index, path=current_path, search=body.search, replace=body.replace))

    tail = segments[-1]
    if SEARCH_MARKER in tail:
        errors.append(
            BlockError(
                len(segments) - 1,
                _filename_in(tail.split(SEARCH_MARKER)[0]) or current_path,
                "block is not terminated by >>>>>>> REPLACE (truncated output?)",
            )
        )

    return ParseResult(edits=tuple(edits), errors=tuple(errors))


def _marker_reason(count: int) -> str:
    """Explain a block whose ``<<<<<<< SEARCH`` marker count is not one."""
    if count == 0:
        return "block has no <<<<<<< SEARCH marker"
    return f"block has {count} <<<<<<< SEARCH markers; expected one"


def _filename_in(header: str) -> str | None:
    """Return the file path a block header names, or None when it names none.

    The last path-shaped line that is not a code fence wins: with a fenced
    block the fence sits between the previous edit and this one's path, and
    with a bare block there is nothing but the path. A leading ``#`` run is
    stripped so the ``### path/to/file.py`` heading the Agentless prompt asks
    for reads as a path.

    Lines that are not path-shaped are skipped rather than returned, so the
    sentence a model writes above its block ("I will now fix the rounding in
    src/app.py:") becomes "this block names no file" instead of becoming the
    filename. Skipping rather than stopping is what lets a real header
    survive a line of prose written under it.
    """
    for line in reversed(header.splitlines()):
        candidate = line.strip()
        if not candidate or candidate.startswith(FENCE):
            continue
        stripped = candidate.lstrip("#").strip()
        if stripped and _is_path_shaped(stripped):
            return stripped
    return None


def _is_path_shaped(candidate: str) -> bool:
    """Return True when ``candidate`` could be a filename rather than prose.

    The format has no quoting, so shape is the only signal there is. One word
    is always taken as a path -- ``Makefile`` and ``src/app.py`` are both
    words, and so is ``../../etc/passwd``, which is refused later by
    containment rather than here. A candidate carrying spaces is taken only
    when it stays within a few words and its last component ends in an
    extension, which is what a filename with a space in it looks like and
    what a sentence about a file does not.
    """
    words = candidate.split()
    if len(words) == 1:
        return True
    if len(words) > _MAX_PATH_WORDS or "  " in candidate or "\t" in candidate:
        return False
    return _has_extension(candidate.rpartition("/")[2])


def _has_extension(name: str) -> bool:
    """Return True when ``name`` ends in a plain ``.ext`` suffix."""
    stem, dot, suffix = name.rpartition(".")
    return bool(dot and stem and suffix.isalnum() and len(suffix) <= _MAX_EXTENSION)


@dataclass(frozen=True)
class _Body:
    """A block body split at its divider, or the reason it could not be."""

    search: str
    replace: str
    reason: str


def _split_body(body: str) -> _Body:
    """Split the text after ``<<<<<<< SEARCH`` into its two sides."""
    lines = body.split("\n")
    if lines[0].strip():
        return _Body("", "", "text follows the <<<<<<< SEARCH marker on its own line")

    # Drop the remainder of the marker line, and the newline that precedes the
    # closing marker. Content sharing a line with the closing marker is kept:
    # it is part of the replacement, malformed only in its line breaks.
    lines = lines[1:]
    if lines and lines[-1] == "":
        lines = lines[:-1]

    dividers = [number for number, line in enumerate(lines) if line.rstrip() == DIVIDER]
    if len(dividers) != 1:
        return _Body("", "", _divider_reason(len(dividers)))

    at = dividers[0]
    search = "\n".join(lines[:at])
    replace = "\n".join(lines[at + 1 :])
    if search == ELISION and replace.strip() == ELISION:
        # `...` is the format's own elision token, so a generation that stalls
        # mid-block emits it on both sides for free. Applying it anchors the
        # search side to a unique line and writes the replacement verbatim,
        # which puts a literal `...` statement into the file and reports the
        # patch as applied -- and in python `...` parses, so the syntax delta
        # blesses it too. It is refused here, where the block is still a block.
        return _Body("", "", "both sides of this block are '...': there is nothing to write")
    return _Body(search, replace, "")


def _divider_reason(count: int) -> str:
    """Explain a block body whose ``=======`` divider count is not one."""
    if count == 0:
        return "block has no ======= divider between the search and replace sides"
    return f"block has {count} ======= dividers; expected one"


# ---------------------------------------------------------------------------
# Applying
# ---------------------------------------------------------------------------


def apply_edits(
    edits: Sequence[Edit],
    file_contents: Mapping[str, str],
    *,
    intervals: Mapping[str, Sequence[tuple[int, int]]] | None = None,
) -> ApplyResult:
    """Apply ``edits`` to ``file_contents``, reporting each edit's outcome.

    Edits to one file are applied in the order given and each sees the result
    of the ones before it. ``intervals`` restricts where an edit for a path
    may match, as 1-based inclusive line ranges over the file's *original*
    content -- the Agentless ``context_segment`` scoping, with the ranges
    tracked as character spans so that an earlier edit changing the file's
    length does not move a later edit's scope out from under it.

    A path with no entry in ``intervals`` (and the ``intervals=None`` case)
    is searched whole, which is the empty-intervals fix.
    """
    scoped_paths = set(intervals) if intervals is not None else set()
    outcomes: list[EditOutcome | None] = [None] * len(edits)
    new_contents: dict[str, str] = {}

    for path, positions in _group_by_path(edits).items():
        original = file_contents.get(path)
        if original is None:
            for position in positions:
                outcomes[position] = EditOutcome(
                    edit=edits[position],
                    status=EditStatus.NO_SUCH_FILE,
                    reason=f"no such file in this patch's scope: {path}",
                )
            continue

        ranges = intervals.get(path) if intervals is not None else None
        content = original
        scopes = _initial_scopes(content, ranges)
        scoped = path in scoped_paths and bool(ranges)

        for position in positions:
            outcome, content, scopes = _apply_one(edits[position], content, scopes, scoped=scoped)
            outcomes[position] = outcome

        if content != original:
            new_contents[path] = content

    # Every slot is filled: the grouping visits each edit exactly once.
    return ApplyResult(
        outcomes=tuple(outcome for outcome in outcomes if outcome is not None),
        new_contents=new_contents,
    )


def _group_by_path(edits: Sequence[Edit]) -> dict[str, list[int]]:
    """Group edit positions by path, preserving first-appearance order."""
    grouped: dict[str, list[int]] = {}
    for position, edit in enumerate(edits):
        grouped.setdefault(edit.path, []).append(position)
    return grouped


def _apply_one(
    edit: Edit,
    content: str,
    scopes: Sequence[tuple[int, int]],
    *,
    scoped: bool,
) -> tuple[EditOutcome, str, list[tuple[int, int]]]:
    """Apply one edit to ``content``, returning the outcome and the new state."""
    padded = _pad(content)
    search, replace, anchor_reason = resolve_elisions(edit.search, edit.replace, padded, scopes)
    if anchor_reason:
        return (
            EditOutcome(edit=edit, status=EditStatus.NO_ANCHOR, reason=anchor_reason),
            content,
            list(scopes),
        )

    needle = _pad(search)
    everywhere = list(_find_all(padded, needle))
    inside = [at for at in everywhere if _within(at, len(needle), scopes)]

    if len(inside) > 1:
        return (
            EditOutcome(
                edit=edit,
                status=EditStatus.AMBIGUOUS,
                reason=f"search text ambiguous ({len(inside)} matches)",
                matches=len(inside),
            ),
            content,
            list(scopes),
        )

    if not inside:
        if everywhere and scoped:
            return (
                EditOutcome(
                    edit=edit,
                    status=EditStatus.OUTSIDE_INTERVALS,
                    reason=(
                        f"search text found {len(everywhere)} times in the file but never "
                        "inside the lines this edit was scoped to"
                    ),
                    matches=len(everywhere),
                ),
                content,
                list(scopes),
            )
        return (
            EditOutcome(edit=edit, status=EditStatus.NOT_FOUND, reason="search text not found"),
            content,
            list(scopes),
        )

    at = inside[0]
    end = at + len(needle)
    replacement = _pad(replace)
    updated = padded[:at] + replacement + padded[end:]
    delta = len(replacement) - len(needle)
    return (
        EditOutcome(edit=edit, status=EditStatus.APPLIED, reason="", matches=1),
        _unpad(updated),
        _shift(scopes, end, delta),
    )


def _pad(text: str) -> str:
    """Wrap ``text`` in newline sentinels so matching is whole-line only."""
    return "\n" + text + "\n"


def _unpad(padded: str) -> str:
    """Undo :func:`_pad`."""
    return padded[1:-1]


def _find_all(haystack: str, needle: str) -> Iterator[int]:
    """Yield every start offset of ``needle``, including overlapping ones.

    Overlapping matters here: consecutive identical lines share the newline
    that separates them, so a non-overlapping scan would report one match
    where there are two and apply an edit that is genuinely ambiguous.
    """
    if not needle:
        return
    at = haystack.find(needle)
    while at != -1:
        yield at
        at = haystack.find(needle, at + 1)


def _within(at: int, length: int, scopes: Sequence[tuple[int, int]]) -> bool:
    """True when the span starting at ``at`` fits entirely inside some scope."""
    return any(start <= at and at + length <= end for start, end in scopes)


def _shift(scopes: Sequence[tuple[int, int]], edited_end: int, delta: int) -> list[tuple[int, int]]:
    """Move every scope boundary at or past ``edited_end`` by ``delta``."""

    def moved(offset: int) -> int:
        return offset + delta if offset >= edited_end else offset

    return [(moved(start), moved(end)) for start, end in scopes]


def _initial_scopes(
    content: str, ranges: Sequence[tuple[int, int]] | None
) -> list[tuple[int, int]]:
    """Turn 1-based inclusive line ranges into character spans of the padded text.

    A span runs from the newline *before* the first line to the newline
    *after* the last one, which is exactly the region a whole-line match may
    occupy. Without ranges the span is the whole padded text.
    """
    padded_length = len(content) + 2
    if not ranges:
        return [(0, padded_length)]

    lines = _line_spans(content)
    spans: list[tuple[int, int]] = []
    for start, end in ranges:
        low = max(1, start)
        high = len(lines) if end == -1 else min(len(lines), end)
        if low > high or low > len(lines):
            continue
        spans.append((lines[low - 1][0] - 1, lines[high - 1][1] + 1))
    return spans


def _line_spans(content: str) -> list[tuple[int, int]]:
    """Return each line's ``(start, end)`` offsets in the padded text."""
    spans: list[tuple[int, int]] = []
    offset = 1
    for line in content.split("\n"):
        spans.append((offset, offset + len(line)))
        offset += len(line) + 1
    return spans


def resolve_elisions(
    search: str,
    replace: str,
    padded: str,
    scopes: Sequence[tuple[int, int]],
) -> tuple[str, str, str]:
    """Expand ``...`` elisions, or explain why this one cannot be expanded.

    Public because the elision rule has to have exactly one owner: anything
    that wants to know which lines an edit really searches for -- applying it
    here, or locating it in :mod:`agentless_mcp.core.patchlint` -- has to ask
    the same question of the same code, or an elided edit means two different
    things depending on who read it.

    Ported from the original's ``parse_for_threedots`` with its two crashes
    guarded: an empty replacement side indexed ``replace[0]``, and a search
    side of exactly ``...`` with no anchor fell through printing a message and
    matching nothing.
    """
    if replace.startswith(ELISION_PREFIX) and len(replace) > len(ELISION_PREFIX):
        replace = replace[len(ELISION_PREFIX) :]

    if search == ELISION:
        if not replace or replace[0].isspace():
            return (
                search,
                replace,
                "elided search block needs a replacement whose first line starts at column 1",
            )
        anchor = _find_anchor(padded, scopes)
        if anchor is None:
            return (
                search,
                replace,
                "elided search block has no unique unindented anchor line in scope",
            )
        return anchor, replace + "\n\n" + anchor, ""

    if search.startswith(ELISION_PREFIX) and len(search) > len(ELISION_PREFIX):
        search = search[len(ELISION_PREFIX) :]

    return search, replace, ""


def _find_anchor(padded: str, scopes: Sequence[tuple[int, int]]) -> str | None:
    """Return an unindented line that occurs exactly once in the whole file.

    Scopes are consulted in order and the first qualifying line wins, so an
    elided edit lands in the first region it was pointed at rather than
    wherever the file happens to have a unique line.
    """
    for start, end in scopes:
        for line in padded[start:end].split("\n"):
            if not line or line[0].isspace():
                continue
            if sum(1 for _ in _find_all(padded, _pad(line))) == 1:
                return line
    return None
