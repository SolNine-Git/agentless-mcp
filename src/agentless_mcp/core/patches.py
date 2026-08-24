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
sentence names no file rather than naming the sentence -- a single word has
to be spelled like a filename to count, because every single word counting
made ``Done!`` a path and handed it to the block below. Filenames are
taken verbatim after stripping: the original wrapped them in quotes and used
``eval()`` to unwrap them at apply time, which is remote code execution
reachable from model output. Neither the quoting nor the ``eval`` is here.

Three behavioural fixes over the original, each pinned by a test:

* **A search runs over the whole file.** The original took per-file location
  intervals and referenced ``original`` and ``replace`` before assignment when
  a file had none, so that path raised ``NameError`` instead of applying
  anything. The intervals are gone rather than repaired: nothing an agent can
  reach ever supplied them, and a scope no caller can express is a second way
  to be wrong about where an edit landed.
* **Every non-applied block reports why.** The original printed
  ``"not replaced"`` to stdout and returned the unchanged content, so a caller
  could not tell a no-op patch from an applied one. Each edit here returns a
  structured outcome: applied, not found, ambiguous with a match count, or
  anchored nowhere.
* **Ambiguity is refused, not guessed.** ``str.replace`` in the original
  rewrote *every* occurrence of the search text. Search text matching more
  than once is now reported with the count and applied nowhere, because which
  one the author meant is not knowable from the block.

Two structural rules are preserved from the original because they are what
make the format work at all. Matching is **whole-line**: both the haystack and
the needle are wrapped in newline sentinels, so a search block can never match
a fragment of a longer line. And ``...`` **elisions** are honoured: a search
block of just ``...`` is anchored to a unique unindented line, and a
leading ``...`` line on either side is dropped. A block that elides *both*
sides describes no change and is refused at parse time.

Nothing here touches the filesystem. Paths are dictionary keys; reading and
writing files, and deciding which paths a caller may name at all, belong to
:mod:`agentless_mcp.application.patch_service`.
"""

from collections import Counter
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from enum import Enum
from typing import Any

SEARCH_MARKER = "<<<<<<< SEARCH"
DIVIDER = "======="
REPLACE_MARKER = ">>>>>>> REPLACE"
FENCE = "```"

# The opening line of the other SEARCH/REPLACE dialect in circulation -- the
# canonical Agentless / SWE-bench ``*** Begin Patch`` form. It is recognised
# only to be named in the zero-block refusal, never parsed.
BEGIN_PATCH_MARKER = "*** Begin Patch"

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

# What a header line may not end on and still be read as a filename: the
# punctuation a sentence ends on, and the delimiter a quoted or bracketed
# phrase closes with. A line ending on one of these is prose, which is a fact
# about the line rather than about the alphabet it is written in -- the test
# this replaced enumerated the characters a path may *contain*, in ASCII, and
# so read every accented or non-Latin filename as prose.
_PROSE_END = frozenset(".,:;!?\"')]}")

# What a header line may not begin with. A filename may open with `.`, `/`,
# `~` or `-`; it does not open with a delimiter. Kept apart from `_PROSE_END`
# because it carries no evidence either way about prose: a line failing here
# is one this cannot read, and `_filename_in` answers those differently.
_PATH_START_REFUSED = frozenset("\"'([{")


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

    Text containing no block at all is an error, not an empty success: "I
    found nothing to parse" and "this parsed to zero edits" are different
    answers, and the first read as the second is how ``*** Begin Patch`` text
    sailed through parse and lint as a silent no-op.
    """
    segments = text.split(REPLACE_MARKER)
    edits: list[Edit] = []
    errors: list[BlockError] = []
    current_path: str | None = None

    for index, segment in enumerate(segments[:-1]):
        parts = segment.split(SEARCH_MARKER)
        named = _filename_in(parts[0])
        if named.reason:
            # Reported against the path this block would have inherited, so
            # the reader can see which file it was about to be given.
            errors.append(BlockError(index, current_path, named.reason))
            continue
        if named.path is not None:
            current_path = named.path

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
                _filename_in(tail.split(SEARCH_MARKER)[0]).path or current_path,
                "block is not terminated by >>>>>>> REPLACE (truncated output?)",
            )
        )

    if not edits and not errors:
        errors.append(BlockError(0, None, _no_blocks_reason(text)))

    return ParseResult(edits=tuple(edits), errors=tuple(errors))


def _no_blocks_reason(text: str) -> str:
    """Say why marker-less text is refused, naming the dialect when it shows."""
    reason = (
        f"no SEARCH/REPLACE blocks found: expected {SEARCH_MARKER}, "
        f"{DIVIDER} and {REPLACE_MARKER} markers"
    )
    if BEGIN_PATCH_MARKER in text:
        return (
            f"{reason}; this looks like {BEGIN_PATCH_MARKER} text -- "
            "rewrite each hunk as a SEARCH/REPLACE block"
        )
    return reason


def _marker_reason(count: int) -> str:
    """Explain a block whose ``<<<<<<< SEARCH`` marker count is not one."""
    if count == 0:
        return "block has no <<<<<<< SEARCH marker"
    return f"block has {count} <<<<<<< SEARCH markers; expected one"


@dataclass(frozen=True)
class _HeaderPath:
    """What a block's header names: a path, nothing, or a line it cannot read.

    Three answers rather than two, because the two the caller used to get
    conflated the only case where inheriting the previous block's path is
    right with the case where it is a guess. A header holding nothing at all
    -- a bare block, or a fence -- is the inheritance the format is built on.
    A header holding a line this looked at and could not read is not: the
    block below it belongs to whatever that line says, and attributing it to
    the file above instead points every finding, and every write, at a file
    the author did not name.

    ``reason`` is non-empty only for that second case.
    """

    path: str | None
    reason: str


def _filename_in(header: str) -> _HeaderPath:
    """Return the file path a block header names, or say why it names none.

    The last path-shaped line that is not a code fence wins: with a fenced
    block the fence sits between the previous edit and this one's path, and
    with a bare block there is nothing but the path. A leading ``#`` run is
    stripped so the ``### path/to/file.py`` heading the Agentless prompt asks
    for reads as a path.

    A line punctuated like prose is skipped rather than returned, so the
    sentence a model writes above its block ("I will now fix the rounding in
    src/app.py:") becomes "this block names no file" instead of becoming the
    filename. Skipping rather than stopping is what lets a real header
    survive a line of prose written under it.

    A line that is neither a path nor prose stops the walk and is reported.
    It is the header this cannot read, and the answer to that is to say so:
    reading it as "no header" made the block inherit the path above it, which
    is how an accented filename came to be silently edited as a different
    file. The trigger there was one over-narrow character class, and a
    character class can be got wrong again; what stops the next one becoming
    a wrong edit is that an unreadable header is refused rather than guessed.
    """
    for line in reversed(header.splitlines()):
        candidate = line.strip()
        if not candidate or candidate.startswith(FENCE):
            continue
        stripped = candidate.lstrip("#").strip()
        if not stripped:
            continue
        verdict = _classify_header(stripped)
        if verdict is _Header.PATH:
            return _HeaderPath(stripped, "")
        if verdict is _Header.PROSE:
            continue
        return _HeaderPath(None, _unreadable_header_reason(stripped))
    return _HeaderPath(None, "")


def _unreadable_header_reason(candidate: str) -> str:
    """Say why a header line was read as neither a path nor prose."""
    return (
        f"the line above this block, {candidate!r}, is neither a path nor a sentence, so "
        "which file this block edits is not stated; put the path on its own line above "
        "<<<<<<< SEARCH"
    )


class _Header(str, Enum):
    """What one candidate header line is, as far as shape can say.

    ``PROSE`` and ``UNREADABLE`` are both "not a path", kept apart because the
    caller does two different things with them. Prose is skipped, so the real
    header written above it still wins -- which is the whole reason a model
    may narrate between its blocks. Unreadable is reported, because the block
    under it would otherwise take the path of the block above it.

    The line between them is positive evidence. A line is prose when
    something about it says so: the punctuation it ends on, more words than a
    filename carries, or a last component with no extension. Absent all of
    that, "not a path" means only that this could not read it, and a parser
    that answers "I could not read this" with "inherit the file above" is how
    an accented filename came to be edited as a different file.
    """

    PATH = "path"
    PROSE = "prose"
    UNREADABLE = "unreadable"


def _classify_header(candidate: str) -> _Header:
    """Say whether a header line is a path, prose, or a line this cannot read.

    The format has no quoting, so shape is the only signal there is. A single
    word is a path unless it is *punctuated* like prose: ``Makefile``,
    ``src/app.py``, ``naïve.py`` and ``../../etc/passwd`` are paths -- the
    last refused later by containment rather than here -- and ``Done!``,
    ``Next:``, ``Also,`` and ``Fixed.`` are not. It is a shape test and not a
    guarantee: a bare English word that is also a legal filename still
    passes, and ``apply_edits`` answers that one with ``no_such_file`` rather
    than writing to it.

    The characters a path may *contain* are deliberately not enumerated. A
    filename is any sequence of non-separator characters, so an enumeration
    written in ASCII read ``src/naive.py`` as a path and ``src/naïve.py`` as
    prose -- and the block under the second one was then attributed to the
    file above it.

    A candidate carrying spaces is a path only when it stays within a few
    words and its last component ends in an extension, which is what a
    filename with a space in it looks like and what a sentence about a file
    does not. Failing either of those is evidence of prose in its own right,
    whatever the line ends on: "Now fixing" and "I will now fix the rounding
    in src/app.py" are sentences with or without their punctuation, and a
    model writing one between two blocks is the ordinary case rather than the
    edge case.

    That leaves one line this can neither read nor call prose: one opening on
    a quote or a bracket. A filename may open with ``.``, ``/``, ``~`` or
    ``-``; it does not open with a delimiter, and a delimiter says the line
    was meant as something the format does not accept.
    """
    if candidate[0] in _PATH_START_REFUSED:
        return _Header.UNREADABLE
    if candidate[-1] in _PROSE_END:
        return _Header.PROSE

    words = candidate.split()
    if len(words) == 1:
        return _Header.PATH
    if len(words) > _MAX_PATH_WORDS or "  " in candidate or "\t" in candidate:
        return _Header.PROSE
    if _has_extension(candidate.rpartition("/")[2]):
        return _Header.PATH
    return _Header.PROSE


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
    crlf = _crlf_reason(lines)
    if crlf:
        return _Body("", "", crlf)

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


def _crlf_reason(lines: Sequence[str]) -> str:
    """Explain a block whose structural lines end in a carriage return, else ''.

    A carriage return on the ``<<<<<<< SEARCH`` line or on the ``=======``
    divider means the patch text itself is CRLF, so every content line carries
    one too and the needle can never match an LF checkout. Left undiagnosed
    this surfaced as ``search text not found`` on every block, which reads as
    a wrong search string and sends the author looking in the wrong place.

    :mod:`agentless_mcp.core.unidiff` refuses a structural carriage return for
    exactly this reason and keeps one inside content verbatim; the same rule
    applies here, so one cause has one diagnosis in both parsers.
    """
    structural = [lines[0], *(line for line in lines if line.rstrip() == DIVIDER)]
    if not any(line.endswith("\r") for line in structural):
        return ""
    return (
        "a structural line ends with a carriage return, so this patch text has CRLF line "
        "endings; convert it to LF and rerun"
    )


def _divider_reason(count: int) -> str:
    """Explain a block body whose ``=======`` divider count is not one."""
    if count == 0:
        return "block has no ======= divider between the search and replace sides"
    return f"block has {count} ======= dividers; expected one"


# ---------------------------------------------------------------------------
# Applying
# ---------------------------------------------------------------------------


def apply_edits(edits: Sequence[Edit], file_contents: Mapping[str, str]) -> ApplyResult:
    """Apply ``edits`` to ``file_contents``, reporting each edit's outcome.

    Edits to one file are applied in the order given and each sees the result
    of the ones before it, and every edit is matched against the whole file.

    The Agentless ``context_segment`` scoping this was ported with -- 1-based
    line ranges per path, tracked as character spans so an earlier edit could
    not move a later one's scope -- is gone. No surface ever supplied it: the
    CLI patch commands took no ranges, the MCP server exposes no patch tool at
    all, and ``patchlint`` matched whole files. What it left behind was a
    serialised status a caller could never receive and sixty lines of offset
    arithmetic only tests ran. An agent that needs a narrower match writes a
    longer SEARCH side, which is in the patch and reviewable; an ambiguous one
    is still refused with its count rather than resolved by a coordinate the
    patch does not carry.
    """
    outcomes: dict[int, EditOutcome] = {}
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

        content = original
        for position in positions:
            outcome, content = _apply_one(edits[position], content)
            outcomes[position] = outcome

        if content != original:
            new_contents[path] = content

    # Every slot is filled: the grouping visits each edit exactly once. Read
    # back by position so a slot the grouping missed raises here, rather than
    # dropping out of the tuple and reading downstream as a smaller patch that
    # succeeded.
    return ApplyResult(
        outcomes=tuple(outcomes[position] for position in range(len(edits))),
        new_contents=new_contents,
    )


def _group_by_path(edits: Sequence[Edit]) -> dict[str, list[int]]:
    """Group edit positions by path, preserving first-appearance order."""
    grouped: dict[str, list[int]] = {}
    for position, edit in enumerate(edits):
        grouped.setdefault(edit.path, []).append(position)
    return grouped


def _apply_one(edit: Edit, content: str) -> tuple[EditOutcome, str]:
    """Apply one edit to ``content``, returning the outcome and the new text."""
    if not edit.search:
        # An empty pre-image means "create this file", and this function never
        # creates one: a path absent from `file_contents` is already reported
        # NO_SUCH_FILE above, so every file reaching here exists. Against an
        # existing file the empty needle pads to "\n\n", matches the first
        # blank line -- or the end of the file when there is none -- and the
        # replacement is written with the outcome reported as `applied`.
        # Reproduced: Edit(search="", replace="INJECTED\n") against a two-line
        # file returned ok=True, matches=1, and appended the text.
        #
        # `core/unidiff` and `core/patchlint` both refuse this already. They
        # are the two modules that cannot write; this is the one that can.
        return (
            EditOutcome(
                edit=edit,
                status=EditStatus.NO_ANCHOR,
                reason=(
                    f"{edit.path}: the SEARCH side is empty, which anchors nowhere in a "
                    f"file that already exists"
                ),
            ),
            content,
        )

    padded = _pad(content)
    whole_file = ((0, len(padded)),)
    search, replace, anchor_reason = resolve_elisions(edit.search, edit.replace, padded, whole_file)
    if anchor_reason:
        return (
            EditOutcome(edit=edit, status=EditStatus.NO_ANCHOR, reason=anchor_reason),
            content,
        )

    needle = _pad(search)
    matches = list(_find_all(padded, needle))

    if len(matches) > 1:
        return (
            EditOutcome(
                edit=edit,
                status=EditStatus.AMBIGUOUS,
                reason=f"search text ambiguous ({len(matches)} matches)",
                matches=len(matches),
            ),
            content,
        )

    if not matches:
        return (
            EditOutcome(edit=edit, status=EditStatus.NOT_FOUND, reason="search text not found"),
            content,
        )

    at = matches[0]
    end = at + len(needle)
    updated = padded[:at] + _pad(replace) + padded[end:]
    return (
        EditOutcome(
            edit=edit,
            status=EditStatus.APPLIED,
            reason=_placement(edit, search, padded, at),
            matches=1,
        ),
        _unpad(updated),
    )


def _placement(edit: Edit, search: str, padded: str, at: int) -> str:
    """Say where a bare ``...`` edit landed; say nothing for a located one.

    A block whose search side is only ``...`` expresses no location at all:
    :func:`_find_anchor` picks the first unindented unique line in the file,
    which is a property of the file rather than anything the author wrote.
    Reporting `applied` and nothing else made the caller accept
    a placement it could not see, so the line the insert went above is named
    here. Every other edit says where it goes by quoting it, and repeating
    that back adds nothing.
    """
    if edit.search != ELISION:
        return ""
    # `at` is the offset of the newline that opens the matched line, so the
    # newlines before it are exactly the lines before it.
    number = padded.count("\n", 0, at) + 1
    return f"inserted above line {number}: {search!r}"


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


def resolve_elisions(
    search: str,
    replace: str,
    padded: str,
    scopes: Sequence[tuple[int, int]],
) -> tuple[str, str, str]:
    """Expand ``...`` elisions, or explain why this one cannot be expanded.

    Public so that the elision rule *can* have exactly one owner. It does not
    have one yet: :func:`agentless_mcp.core.patchlint._locate` matches the
    search text as written and never calls this, so an elided edit anchors in
    the applier and anchors nowhere in the linter. That function's own
    docstring names the gap. This one exists to be called by it, not to claim
    it already is.

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

    Uniqueness is counted once, over the whole file, instead of running a
    full-file scan per candidate. Same answer: a padded line matches padded
    text exactly where that whole line occurs, and consecutive identical
    lines -- the overlap case :func:`_find_all` exists for -- are two entries
    in the count either way. The early return bounded the old scan in the
    common case, but a file whose leading lines are all indented or all
    duplicated read the whole file once per line.
    """
    occurrences = Counter(padded.split("\n"))
    for start, end in scopes:
        for line in padded[start:end].split("\n"):
            if not line or line[0].isspace():
                continue
            if occurrences[line] == 1:
                return line
    return None
