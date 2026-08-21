"""Reading an existing unified diff as edits the patch checks can consume.

``lint --candidates`` takes what a model wrote: SEARCH/REPLACE blocks or an
``edits.json``. This takes what a repository already has -- the output of
``git diff``, or the body of a ``git format-patch`` -- so the same
deterministic checks can be pointed at a branch or a pull request that exists
rather than only at a candidate somebody is about to apply.

Nothing here touches the filesystem or shells anything: text in,
:class:`agentless_mcp.core.patches.Edit` values out, exactly like
:mod:`agentless_mcp.core.patches`. Reading the file and scanning the repository
belong to the application layer.

**One hunk is one edit.** The search side is the hunk's pre-image -- its
context and removed lines -- and the replace side its post-image, its context
and added lines, both with the full context retained. Trimming context would be
the wrong economy: context is what makes a pre-image unique in the file, and
both :func:`agentless_mcp.core.patchlint._locate` and
:func:`agentless_mcp.core.patches.apply_edits` refuse an ambiguous match, so a
trimmed hunk buys nothing and loses the anchor.

**Refusals are returned, not raised.** The result carries the same
:class:`agentless_mcp.core.patches.ParseResult` the SEARCH/REPLACE grammar
produces, so a diff this module will not read reaches the report through the
coverage-gap path a malformed block already travels, and ``lint`` keeps its
exit-0 contract. :attr:`DiffParse.notes` is the other half: a binary file or a
mode-only change has nothing to check but must not vanish, so it travels as a
note the caller renders beside the findings rather than as a dropped section.

Lines before the first file section are a prologue -- a commit message and
diffstat from ``git format-patch``, a ``git log -p`` header -- and are passed
over. They are not diff content. A file section begins at ``diff --git`` or at
a ``---`` line naming a file.

What is refused, and why each is a refusal rather than a silent skip:

* **A rename or a copy.** :class:`Edit` carries one path. Attributing a renamed
  file's changes to either end of the rename points every finding at a file
  that does not have them. Guarded on the invariant rather than on one header
  spelling: the ``rename``/``copy`` headers *and* the ``---``/``+++`` pair
  naming two different paths both trip it, because git emits the pure rename
  with no ``---``/``+++`` lines at all and the rename-with-edits with both.
* **A path git C-quoted** (``"a/na\\303\\257ve.py"``). Decoding those escapes
  wrong mis-attributes findings silently, which is worse than not reading the
  diff at all.
* **A zero-context hunk into a file that already exists** (``diff -U0``). Its
  pre-image is empty, and an empty search anchors nowhere: ``_locate`` gives up
  and ``apply_edits`` pads the empty needle into ``"\\n\\n"``, which matches
  every blank line in the file. A *new* file's hunk is the one empty pre-image
  that is safe, because the file is absent from the base tree and every check
  that would use the anchor reports a coverage gap before reaching it.
* **A combined (merge) diff.** A different grammar, with two pre-images per
  hunk and no single edit to map them onto.
* **A carriage return on a structural line.** That means the patch file itself
  is CRLF, and guessing which ``\\r`` is content and which is line ending is how
  a parser corrupts search text. A carriage return inside *content* is kept
  verbatim: the file in the tree has it too, so the pre-image still matches.
* **Hunk line counts that disagree with the ``@@`` header.** The truncation
  guard, and also what makes the parse unambiguous: hunk content is consumed by
  count, so a context line that itself begins ``--- `` cannot be mistaken for
  the next file's header.
"""

import re
from collections.abc import Mapping, Sequence
from dataclasses import dataclass

from agentless_mcp.core.patches import BlockError, Edit, ParseResult

_DIFF_GIT = "diff --git "
_COMBINED = ("diff --cc ", "diff --combined ")
_OLD = "--- "
_NEW = "+++ "
_DEV_NULL = "/dev/null"
_NO_NEWLINE = "\\ "
_BINARY_LITERAL = "GIT binary patch"
_BINARY_TEXT = "Binary files "

# Extended headers that say a path changed identity. Consumed here so the
# section still parses, and answered by the rename refusal below.
_RENAME_HEADERS = ("rename from ", "rename to ", "copy from ", "copy to ")

# Extended headers that carry no content implication for any check.
_IGNORED_HEADERS = (
    "index ",
    "similarity index ",
    "dissimilarity index ",
    "old mode ",
    "new mode ",
    "new file mode ",
    "deleted file mode ",
)

_HUNK_HEADER = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")

# What the caller is told to do about every misorientation. The diagnosis alone
# sends a reader back to the guide; naming the fix closes the loop.
_REMEDY = (
    "point --repo at a checkout of the diff's base (for a branch, its merge-base "
    "with the target) and rerun"
)


@dataclass(frozen=True)
class DiffNote:
    """One section that parsed but holds nothing any check can read.

    A binary file or a mode-only change. Not an error -- the diff is fine and
    the section is real -- but not a silent skip either, which is why it is a
    value the caller has to render rather than a section that disappears.
    """

    path: str
    reason: str


@dataclass(frozen=True)
class DiffParse:
    """One diff's edits and refusals, plus what it held that has no content.

    ``result`` is the same type the SEARCH/REPLACE grammar returns so that both
    input formats reach the report through one downstream. ``notes`` are the
    sections that parsed but have nothing to check, reported beside the findings
    rather than instead of them.
    """

    result: ParseResult
    notes: tuple[DiffNote, ...]


def parse_unified_diff(text: str) -> DiffParse:
    """Parse a unified diff into one edit per hunk, refusing what it cannot map."""
    lines = _lines(text)
    edits: list[Edit] = []
    errors: list[BlockError] = []
    notes: list[DiffNote] = []
    at = 0
    ordinal = 0
    sections = 0

    while at < len(lines):
        line = lines[at]
        if line.startswith(_COMBINED):
            return _refused(
                None,
                "this is a combined (merge) diff, which names two pre-images per hunk; "
                "lint the diff against one parent instead",
            )
        if not _opens_section(line):
            at += 1
            continue

        sections += 1
        section = _file_section(lines, at, ordinal)
        edits.extend(section.edits)
        errors.extend(section.errors)
        notes.extend(section.notes)
        ordinal = section.ordinal
        at = section.next

    if sections == 0:
        return _refused(
            None,
            "no file sections found: this does not look like a unified diff "
            "(expected a 'diff --git' or a '---' header naming a file)",
        )
    return DiffParse(ParseResult(edits=tuple(edits), errors=tuple(errors)), tuple(notes))


def orientation(edits: Sequence[Edit], texts: Mapping[str, str]) -> tuple[BlockError, ...]:
    """Report every edit whose pre-image is not in the tree the checks will use.

    The checks compare a patch against the repository as it stands, so they are
    only meaningful when that repository is the diff's *base*. Linting a diff
    that is already applied does not fail loudly; it reports a false
    ``shadowing`` warning for every top-level symbol the diff adds, because the
    symbol the patch "introduces" is already in the file. This is the guard that
    turns that into a stated coverage gap.

    It keys on the precondition the checks actually need -- is this edit's
    pre-image findable in this text -- rather than on any proxy for tree
    identity. Uniqueness is deliberately not required: an ambiguous pre-image is
    already :func:`agentless_mcp.core.patchlint._locate`'s rule to apply, and a
    second copy of it here is what would drift.

    A path with no text is not judged at all. ``texts`` holds only the files the
    scan parsed, so a diff touching a README would otherwise be refused for
    being absent from a repository that has it.
    """
    problems: list[BlockError] = []
    reported: set[str] = set()
    for edit in edits:
        if edit.path in reported:
            continue
        reason = _misoriented(edit, texts.get(edit.path))
        if reason is None:
            continue
        reported.add(edit.path)
        problems.append(BlockError(index=edit.index, path=edit.path, reason=reason))
    return tuple(problems)


def _misoriented(edit: Edit, text: str | None) -> str | None:
    """Say how this edit disagrees with the tree, or None when it agrees."""
    if text is None:
        return None

    if not edit.search:
        return (
            f"{edit.path} is added by this diff but already exists in --repo, so --repo is "
            f"not the diff's base; {_REMEDY}"
        )
    if _present(text, edit.search):
        return None
    if edit.replace and _present(text, edit.replace):
        return (
            f"{edit.path} already contains this hunk's post-image, so the diff is already "
            f"applied to --repo and the checks would compare the change against itself; "
            f"{_REMEDY}"
        )
    return (
        f"neither this hunk's pre-image nor its post-image is in {edit.path} as --repo has "
        f"it, so this diff is not against --repo; {_REMEDY}"
    )


def _present(text: str, block: str) -> bool:
    """True when ``block`` occurs in ``text`` on whole-line boundaries."""
    return ("\n" + text + "\n").find("\n" + block + "\n") >= 0


# ---------------------------------------------------------------------------
# Sections
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class _Section:
    """One file's worth of diff, and where the next one starts."""

    edits: tuple[Edit, ...]
    errors: tuple[BlockError, ...]
    notes: tuple[DiffNote, ...]
    ordinal: int
    next: int


@dataclass(frozen=True)
class _Side:
    """One side of a ``---``/``+++`` pair: its path, or why it has none.

    ``path`` is None for ``/dev/null``, which is how the format spells "this
    side does not exist" -- an addition on the old side, a deletion on the new.
    ``reason`` is non-empty only when the side could not be read at all.
    """

    path: str | None
    reason: str


def _lines(text: str) -> list[str]:
    """Split a diff into lines, dropping the empty tail a final newline leaves."""
    lines = text.split("\n")
    if lines and lines[-1] == "":
        lines.pop()
    return lines


def _opens_section(line: str) -> bool:
    """True when this line begins a file section.

    Only reached outside a hunk. Hunk content is consumed by the count in its
    ``@@`` header, so a removed line that happens to read ``--- something``
    never gets here.
    """
    return line.startswith(_DIFF_GIT) or (line.startswith(_OLD) and len(line) > len(_OLD))


def _refused(path: str | None, reason: str) -> DiffParse:
    """Return a parse that read nothing and says why."""
    return DiffParse(ParseResult(edits=(), errors=(BlockError(0, path, reason),)), ())


def _section_error(path: str | None, ordinal: int, at: int, reason: str) -> _Section:
    """Return a section that yielded one refusal and consumed to ``at``."""
    return _Section((), (BlockError(ordinal, path, reason),), (), ordinal + 1, at)


def _crlf(line: str) -> str | None:
    """Explain a structural line that ends in a carriage return, or None."""
    if not line.endswith("\r"):
        return None
    return (
        f"a structural line ends with a carriage return, so this patch file has CRLF line "
        f"endings: {line.rstrip()!r}; convert it to LF and rerun"
    )


@dataclass(frozen=True)
class _Headers:
    """One file section's header block, read but not yet judged.

    ``reason`` non-empty means the block itself could not be read; ``binary``
    means it said there is no text here. Both are decided by
    :func:`_file_section`, which owns what a section *means*, so that this stays
    a scanner.

    ``git_section`` records whether the section opened with ``diff --git``,
    which is what says where it *ends*: such a section runs to the next
    ``diff --git`` and its own ``---`` line is part of it, so a refusal raised
    before that line must not resume on it and read the same section twice.
    """

    named: str | None
    old_raw: str | None
    new_raw: str | None
    renamed: bool
    binary: bool
    reason: str
    next: int
    git_section: bool


def _path_line(
    line: str,
    old_raw: str | None,
    new_raw: str | None,
) -> tuple[str | None, str | None] | None:
    """Return the updated ``('---', '+++')`` raw pair, or None for other lines.

    Only the *first* header of each kind is read. A second ``---`` belongs to
    the next section, which is what ends this one.
    """
    if line.startswith(_OLD) and old_raw is None:
        return line[len(_OLD) :], new_raw
    if line.startswith(_NEW) and new_raw is None:
        return old_raw, line[len(_NEW) :]
    return None


def _headers(lines: Sequence[str], at: int) -> _Headers:
    """Scan one file section's header lines, stopping at its first hunk."""
    i = at
    named: str | None = None
    old_raw: str | None = None
    new_raw: str | None = None
    renamed = False
    git_section = lines[i].startswith(_DIFF_GIT)

    if git_section:
        crlf = _crlf(lines[i])
        if crlf is not None:
            return _Headers(None, None, None, False, False, crlf, i + 1, True)
        named = _diff_git_path(lines[i])
        i += 1

    while i < len(lines):
        line = lines[i]
        if line.startswith("@@"):
            break
        pair = _path_line(line, old_raw, new_raw)
        if pair is not None:
            crlf = _crlf(line)
            if crlf is not None:
                return _Headers(named, None, None, False, False, crlf, i + 1, git_section)
            old_raw, new_raw = pair
        elif line == _BINARY_LITERAL or line.startswith(_BINARY_TEXT):
            return _Headers(named, old_raw, new_raw, renamed, True, "", i + 1, git_section)
        elif line.startswith(_RENAME_HEADERS):
            renamed = True
        elif line.startswith(_IGNORED_HEADERS):
            pass
        elif line == "" or _opens_section(line):
            # The section ended without hunks: a mode-only or rename-only entry,
            # or the blank line git leaves after a binary payload.
            break
        else:
            reason = f"unrecognised header line in this file's diff: {line!r}"
            return _Headers(named, old_raw, new_raw, renamed, False, reason, i + 1, git_section)
        i += 1

    return _Headers(named, old_raw, new_raw, renamed, False, "", i, git_section)


def _file_section(lines: Sequence[str], at: int, ordinal: int) -> _Section:
    """Parse one file's headers and hunks, starting at ``at``."""
    read = _headers(lines, at)
    resume = _skip_to_next_section(lines, read.next, git_only=read.git_section)

    if read.reason:
        return _section_error(read.named, ordinal, resume, read.reason)
    if read.binary:
        note = DiffNote(read.named or "", "binary, so there is no text to check")
        return _Section((), (), (note,), ordinal, resume)
    if read.renamed:
        return _section_error(
            read.named,
            ordinal,
            resume,
            "this section renames or copies a file, which one edit cannot express because an "
            "edit carries a single path; lint the content change separately from the rename",
        )

    if read.old_raw is None or read.new_raw is None:
        return _without_pair(read, ordinal, resume)
    return _paths_then_hunks(lines, read, ordinal, resume)


def _without_pair(read: _Headers, ordinal: int, resume: int) -> _Section:
    """Judge a section that carries no complete ``---``/``+++`` pair.

    Both headers absent is the mode-only change git spells that way. One absent
    is a diff that states it changes something without stating what.
    """
    if read.old_raw is None and read.new_raw is None:
        if read.named is None:
            return _section_error(
                None,
                ordinal,
                resume,
                "this section names no file: no '---'/'+++' pair and no 'diff --git' header",
            )
        note = DiffNote(read.named, "mode change only, so there is no content to check")
        return _Section((), (), (note,), ordinal, resume)
    return _section_error(
        read.named,
        ordinal,
        resume,
        "this section has only one of the '---' and '+++' headers, so which file it "
        "changes is not stated",
    )


def _paths_then_hunks(
    lines: Sequence[str],
    read: _Headers,
    ordinal: int,
    resume: int,
) -> _Section:
    """Resolve a section's two paths into one, then read its hunks."""
    old = _side(read.old_raw or "", "a")
    new = _side(read.new_raw or "", "b")
    for side in (old, new):
        if side.reason:
            return _section_error(read.named, ordinal, resume, side.reason)

    if old.path is not None and new.path is not None and old.path != new.path:
        return _section_error(
            read.named,
            ordinal,
            resume,
            f"this section renames {old.path} to {new.path}, which one edit cannot express "
            "because an edit carries a single path; lint the content change separately",
        )

    path = new.path if new.path is not None else old.path
    if path is None:
        return _section_error(
            read.named,
            ordinal,
            resume,
            "both sides of this section are /dev/null, so it names no file at all",
        )
    return _hunks(lines, read.next, _Target(path, old.path is None), ordinal, resume)


def _diff_git_path(line: str) -> str | None:
    """Return the path a ``diff --git a/x b/x`` line names, when it is plain.

    Used only to attribute a refusal or a note to a file. The authoritative
    paths are the ``---``/``+++`` pair; this is a label, and a header it cannot
    read confidently yields None rather than a guess.
    """
    rest = line[len(_DIFF_GIT) :]
    if rest.startswith('"'):
        return None
    _, separator, tail = rest.partition(" b/")
    if not separator:
        return None
    return tail or None


def _side(raw: str, prefix: str) -> _Side:
    """Read one ``---``/``+++`` path, stripping the ``a/``/``b/`` git adds."""
    value = raw.split("\t", maxsplit=1)[0].rstrip()
    if value.startswith('"'):
        return _Side(
            None,
            f"the diff names a C-quoted path ({value}); this reader does not decode those "
            "escapes, because decoding one wrong attributes findings to the wrong file",
        )
    if value == _DEV_NULL:
        return _Side(None, "")
    head = prefix + "/"
    if value.startswith(head):
        value = value[len(head) :]
    if not value:
        return _Side(None, "the diff has an empty path on one side of its '---'/'+++' pair")
    return _Side(value, "")


def _skip_to_next_section(lines: Sequence[str], at: int, *, git_only: bool) -> int:
    """Return where the section containing ``at`` ends.

    ``git_only`` is the difference between the two diff dialects. A
    ``diff --git`` section runs to the next ``diff --git``, so its own ``---``
    line does not end it; a plain ``diff -u`` section has no such opener and
    ends at the next ``---``.
    """
    i = at
    while i < len(lines):
        line = lines[i]
        if line.startswith(_DIFF_GIT) or (not git_only and _opens_section(line)):
            return i
        i += 1
    return len(lines)


# ---------------------------------------------------------------------------
# Hunks
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class _Target:
    """The file one section's hunks belong to, and whether it is a new one."""

    path: str
    new_file: bool


def _hunks(
    lines: Sequence[str],
    at: int,
    target: _Target,
    ordinal: int,
    resume: int,
) -> _Section:
    """Parse every hunk of one file section into one edit each.

    ``resume`` is where this section ends, computed once by :func:`_file_section`
    so that a refusal anywhere inside it lands in the same place. A hunk body may
    hold a line that reads like a header, so a refusal must never go looking for
    the next section from inside one.
    """
    edits: list[Edit] = []
    i = at
    index = ordinal

    if i >= len(lines) or not lines[i].startswith("@@"):
        return _section_error(
            target.path,
            index,
            resume,
            f"{target.path} has a file header but no hunks, so there is no change to read",
        )

    while i < len(lines) and lines[i].startswith("@@"):
        outcome, i = _hunk(lines, i, target, index)
        if isinstance(outcome, BlockError):
            return _Section(tuple(edits), (outcome,), (), index + 1, resume)
        edits.append(outcome)
        index += 1

    return _Section(tuple(edits), (), (), index, i)


def _hunk(
    lines: Sequence[str],
    at: int,
    target: _Target,
    index: int,
) -> tuple[Edit | BlockError, int]:
    """Read one hunk into an edit, or refuse it, and say where the next line is.

    The index returned alongside a refusal is only a forward step; the caller
    replaces it with the section's own end.
    """
    header = lines[at]
    counts = _hunk_counts(header, target)
    if isinstance(counts, str):
        return BlockError(index, target.path, counts), at + 1

    body = _hunk_body(lines, at + 1, counts, target.path, header)
    if isinstance(body, str):
        return BlockError(index, target.path, body), at + 1

    return (
        Edit(index=index, path=target.path, search=body.search, replace=body.replace),
        body.next,
    )


def _hunk_counts(header: str, target: _Target) -> tuple[int, int] | str:
    """Return this hunk's ``(pre-image, post-image)`` line counts, or a refusal.

    An omitted count is 1, which is the format's own spelling for a one-line
    side (``@@ -3 +3 @@``).
    """
    crlf = _crlf(header)
    if crlf is not None:
        return crlf

    match = _HUNK_HEADER.match(header)
    if match is None:
        return (
            f"{target.path} has a hunk header that is not '@@ -old,count +new,count @@': {header!r}"
        )

    old_count = int(match.group(2)) if match.group(2) is not None else 1
    new_count = int(match.group(4)) if match.group(4) is not None else 1

    if old_count == 0 and not target.new_file:
        return (
            f"{target.path} has a zero-context hunk ({header.strip()}) into a file that "
            "already exists, so the hunk has no pre-image to anchor to; regenerate the diff "
            "with context lines (drop -U0)"
        )
    return old_count, new_count


@dataclass(frozen=True)
class _Body:
    """One hunk's two sides, and the first line after it."""

    search: str
    replace: str
    next: int


def _hunk_body(
    lines: Sequence[str],
    at: int,
    counts: tuple[int, int],
    path: str,
    header: str,
) -> _Body | str:
    """Consume exactly the lines the hunk header declares, or say why not.

    Consuming by count rather than by scanning for the next header is what
    makes the whole parse unambiguous: a context line that itself reads
    ``--- something`` is content here, never the next file's header.
    """
    search: list[str] = []
    replace: list[str] = []
    old_left, new_left = counts
    i = at

    while old_left or new_left:
        if i >= len(lines):
            return (
                f"{path}: hunk {header.strip()} ends after part of its body, with "
                f"{old_left} pre-image and {new_left} post-image line(s) still to come "
                "(truncated diff?)"
            )
        line = lines[i]
        if line.startswith(_NO_NEWLINE):
            i += 1
            continue

        # A context line for an empty line is a single space, which editors and
        # mail clients strip to nothing. Reading a bare empty line as context is
        # the tolerance every patch reader has; any other shortfall is refused.
        marker = " " if line == "" else line[0]
        content = "" if line == "" else line[1:]

        if marker == " ":
            fits = bool(old_left and new_left)
            old_left, new_left = old_left - 1, new_left - 1
            search.append(content)
            replace.append(content)
        elif marker == "-":
            fits = bool(old_left)
            old_left -= 1
            search.append(content)
        elif marker == "+":
            fits = bool(new_left)
            new_left -= 1
            replace.append(content)
        else:
            return (
                f"{path}: hunk {header.strip()} has a body line that is not ' ', '-', '+' or "
                f"'\\': {line!r}"
            )
        if not fits:
            return (
                f"{path}: hunk {header.strip()} declares line counts its body does not match "
                "(truncated or hand-edited diff?)"
            )
        i += 1

    while i < len(lines) and lines[i].startswith(_NO_NEWLINE):
        i += 1

    return _Body("\n".join(search), "\n".join(replace), i)
