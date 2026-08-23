"""The write side: parse, syntax-check, apply and normalise SEARCH/REPLACE edits.

This service is the whole write surface of the package and it is reachable
from the CLI only. The MCP server never wires it, deliberately: an MCP tool
that writes to a repository is a tool an agent can be talked into using by the
contents of that repository, and the locked plan puts every write behind an
explicit Bash invocation instead.

Four operations, each a step an agent can run and inspect on its own:

* ``parse`` turns model output into edits plus per-block errors.
* ``check`` applies them in memory and reports, per file, whether the result
  parses no worse than the original did.
* ``apply`` materialises a detached worktree, writes there, and returns the
  unified diff. The caller's checkout is not written to at all unless they ask
  for ``in_place``, which additionally requires a clean tree.
* ``normalize`` returns the AST-equivalence key of the change, so several
  candidate patches can be clustered before any of them is run.

Every path a patch names crosses ``contained_path`` against the repository
root before anything opens it, and the path recorded in every outcome is the
repository-relative form that check produced -- not the string the patch
carried. A block naming ``../../etc/passwd`` or an absolute path outside the
root is a :class:`SecurityRefusal`, and two blocks naming the same file two
different ways are one file.

The base content an edit is matched against differs by mode, and the receipt
says which was used. In worktree mode it is HEAD, because that is what the
worktree contains and what the resulting diff is against. In ``in_place`` mode
it is the working tree, which the clean-tree requirement makes identical to
HEAD anyway.

Two rules govern the write itself, and both are all-or-nothing:

* **A file whose bytes are not UTF-8 is not edited.** The shared reader
  decodes lossily on purpose; this service reads strictly, because writing a
  lossy decode back rewrites every undecodable byte in the file as U+FFFD --
  in regions no edit named.
* **A patch that did not fully apply writes nothing.** ``new_contents`` holds
  every file a *successful* edit touched, so writing it on a failed patch
  leaves a checkout carrying an arbitrary prefix of the change. The write
  stages every file first and moves them into place only once all of them are
  staged, so an ``OSError`` part-way through is an error, not a half-patch.
"""

import json
import os
import shutil
import stat
import tempfile
from collections.abc import Iterable, Mapping, Sequence
from contextlib import suppress
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core import normalize, sandbox
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.normalize import SyntaxVerdict
from agentless_mcp.core.patches import (
    ApplyResult,
    Edit,
    EditOutcome,
    EditStatus,
    ParseResult,
    apply_edits,
    parse_blocks,
)
from agentless_mcp.util.errors import AgentlessError
from agentless_mcp.util.fslimits import BoundedRead, contained_path, read_bounded

EDITS_KEY = "edits"

# What the write side reports for a file it will not decode. `read_bounded`
# maps every undecodable byte to U+FFFD, so the replacement character is the
# necessary sign that a decode was lossy -- and a strict decode of the bytes
# is what decides, which keeps a file that genuinely contains U+FFFD editable.
NOT_UTF8 = "not UTF-8 text: editing it would rewrite the bytes this tool cannot decode"
REPLACEMENT_CHAR = "\ufffd"

# The reason for the one unread file that is not a readability problem at all.
# Written and matched in one place, because "the file is not there" and "the
# file is there and could not be read" are different answers to the caller.
NO_SUCH_FILE = "no such file in this repository"

# The suffix a staged write carries until every file in the patch is staged.
STAGING_SUFFIX = ".agentless-mcp-staged"

# The suffix on an original file while its replacement is being committed.
# Both paths begin with a random component created exclusively by ``mkstemp``;
# the suffix is diagnostic only and is never used to discover a file later.
BACKUP_SUFFIX = ".agentless-mcp-backup"


@dataclass(frozen=True)
class _Sources:
    """The files a patch touches, and the reason for each one it could not read."""

    contents: dict[str, str]
    unreadable: dict[str, str]


@dataclass(frozen=True)
class FileCheck:
    """One edited file's syntax verdict, or the reason there is none."""

    path: str
    verdict: SyntaxVerdict | None
    error: str = ""

    @property
    def ok(self) -> bool:
        """True when the file was read and parses no worse than before."""
        return self.verdict is not None and self.verdict.ok

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this file check."""
        record: dict[str, Any] = {"path": self.path, "ok": self.ok}
        if self.verdict is not None:
            record["verdict"] = self.verdict.as_dict()
        if self.error:
            record["error"] = self.error
        return record


@dataclass(frozen=True)
class CheckReport:
    """What a syntax check found: per-file verdicts and per-edit outcomes."""

    files: tuple[FileCheck, ...]
    result: ApplyResult

    @property
    def ok(self) -> bool:
        """True when every edit applied and every file still parses."""
        return self.result.ok and all(check.ok for check in self.files)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {
            "ok": self.ok,
            "files": [check.as_dict() for check in self.files],
            **self.result.as_dict(),
        }

    def summary_line(self) -> str:
        """Return the one-line summary for the receipt on stderr."""
        applied = sum(1 for outcome in self.result.outcomes if outcome.applied)
        clean = sum(1 for check in self.files if check.ok)
        return (
            f"checked {applied} of {len(self.result.outcomes)} edits across "
            f"{len(self.files)} files; {clean} parse no worse than before"
        )


@dataclass(frozen=True)
class ApplyReport:
    """The diff a patch produces, with the outcome of every edit behind it."""

    diff: str
    result: ApplyResult
    in_place: bool
    base: str

    @property
    def ok(self) -> bool:
        """True when every edit applied."""
        return self.result.ok

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {
            "diff": self.diff,
            "in_place": self.in_place,
            "base": self.base,
            **self.result.as_dict(),
        }

    def summary_line(self) -> str:
        """Return the one-line summary for the receipt on stderr."""
        applied = sum(1 for outcome in self.result.outcomes if outcome.applied)
        where = "the working tree" if self.in_place else "a scratch worktree"
        return (
            f"applied {applied} of {len(self.result.outcomes)} edits to "
            f"{len(self.result.new_contents)} files in {where}, against {self.base}"
        )


@dataclass(frozen=True)
class NormalizeReport:
    """The AST-equivalence key of a patch, and the per-file keys behind it."""

    key: str
    file_keys: dict[str, str]
    result: ApplyResult

    @property
    def ok(self) -> bool:
        """True when every edit applied, so the key covers the whole patch."""
        return self.result.ok

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {"key": self.key, "file_keys": self.file_keys, **self.result.as_dict()}

    def summary_line(self) -> str:
        """Return the one-line summary for the receipt on stderr."""
        return f"equivalence key over {len(self.file_keys)} files"


def load_edits(text: str) -> ParseResult:
    """Read either an ``edits.json`` document or raw SEARCH/REPLACE blocks.

    The two forms are told apart by the first non-space character, so
    ``patch parse | patch check`` and ``patch check`` on raw model output are
    the same command. The JSON form is parsed strictly: every field it is read
    for must be present and of the right type, because a document missing
    ``search`` is a caller bug and must not read downstream as an empty search
    that matches the start of a file.
    """
    if not text.lstrip().startswith("{"):
        return parse_blocks(text)

    try:
        document = json.loads(text)
    except json.JSONDecodeError as exc:
        message = f"edits document is not valid JSON: {exc}"
        raise AgentlessError(message) from exc

    if not isinstance(document, dict) or EDITS_KEY not in document:
        message = f"edits document must be a JSON object with an '{EDITS_KEY}' list"
        raise AgentlessError(message)

    entries = document[EDITS_KEY]
    if not isinstance(entries, list):
        message = f"'{EDITS_KEY}' must be a list of edit objects"
        raise AgentlessError(message)

    return ParseResult(
        edits=tuple(_edit_from(entry, position) for position, entry in enumerate(entries)),
        errors=(),
    )


def _edit_from(entry: object, position: int) -> Edit:
    """Turn one JSON edit object into an :class:`Edit`, or refuse it."""
    if not isinstance(entry, dict):
        message = f"edit {position} is not a JSON object"
        raise AgentlessError(message)

    values: dict[str, str] = {}
    for field in ("path", "search", "replace"):
        value = entry.get(field)
        if not isinstance(value, str):
            message = f"edit {position} is missing a string '{field}'"
            raise AgentlessError(message)
        values[field] = value

    # `index` labels the block in reports and nothing keys on it, so an entry
    # that omits it takes its position in the list rather than being refused.
    index = entry.get("index", position)
    if not isinstance(index, int):
        message = f"edit {position} has a non-integer 'index'"
        raise AgentlessError(message)

    return Edit(
        index=index,
        path=values["path"],
        search=values["search"],
        replace=values["replace"],
    )


class PatchService:
    """Parse, check, apply and normalise SEARCH/REPLACE patches for one repo."""

    def __init__(self, extractor: TreeSitterExtractor) -> None:
        self._extractor = extractor

    # ------------------------------------------------------------------
    # Operations
    # ------------------------------------------------------------------

    def parse(self, text: str) -> ParseResult:
        """Parse patch text into edits and per-block errors."""
        return parse_blocks(text)

    def check(
        self,
        edits: Sequence[Edit],
        ctx: RepoContext,
        *,
        intervals: Mapping[str, Sequence[tuple[int, int]]] | None = None,
    ) -> CheckReport:
        """Apply ``edits`` in memory and report each edited file's syntax delta."""
        scoped = self._canonical(ctx, edits)
        sources = self._read(ctx.root, scoped)
        result = apply_edits(scoped, sources.contents, intervals=intervals)

        checks = [
            FileCheck(path=path, verdict=None, error=reason)
            for path, reason in sorted(sources.unreadable.items())
        ]
        checks.extend(
            FileCheck(
                path=path,
                verdict=normalize.syntax_delta(
                    sources.contents[path], new, self._language_of(path)
                ),
            )
            for path, new in sorted(result.new_contents.items())
        )
        return CheckReport(files=tuple(checks), result=result)

    def apply(
        self,
        edits: Sequence[Edit],
        ctx: RepoContext,
        *,
        in_place: bool = False,
        intervals: Mapping[str, Sequence[tuple[int, int]]] | None = None,
    ) -> ApplyReport:
        """Apply ``edits`` and return the unified diff they produce.

        The default path writes into a throwaway worktree at HEAD and leaves
        the caller's checkout untouched. ``in_place`` writes to the checkout
        and is refused on a dirty tree, because the diff it returns would
        otherwise mix the patch with work the caller had already started.

        Both refusals -- a path outside the root, a dirty tree -- happen
        before a worktree exists, so a patch that is never going to be applied
        does not cost a checkout copy first.

        The write is all or nothing: a patch whose edits did not all land
        writes no file and returns an empty diff, and a write that fails
        part-way raises rather than leaving a prefix of the patch on disk.
        """
        scoped = self._canonical(ctx, edits)
        if in_place:
            self._require_clean(ctx)
            return self._apply_at(ctx, ctx.root, scoped, intervals=intervals, in_place=True)

        with sandbox.worktree(ctx.root) as tree:
            return self._apply_at(ctx, tree, scoped, intervals=intervals, in_place=False)

    def normalize(
        self,
        edits: Sequence[Edit],
        ctx: RepoContext,
        *,
        intervals: Mapping[str, Sequence[tuple[int, int]]] | None = None,
    ) -> NormalizeReport:
        """Return the AST-equivalence key of the change ``edits`` describe."""
        scoped = self._canonical(ctx, edits)
        sources = self._read(ctx.root, scoped)
        result = apply_edits(scoped, sources.contents, intervals=intervals)

        changes = {path: (sources.contents[path], new) for path, new in result.new_contents.items()}
        file_keys = {
            path: normalize.file_key(old, new, self._language_of(path))
            for path, (old, new) in sorted(changes.items())
        }
        return NormalizeReport(
            key=normalize.equivalence_key(changes, self._language_of),
            file_keys=file_keys,
            result=result,
        )

    # ------------------------------------------------------------------
    # Internals
    # ------------------------------------------------------------------

    def _apply_at(
        self,
        ctx: RepoContext,
        base: Path,
        edits: Sequence[Edit],
        *,
        intervals: Mapping[str, Sequence[tuple[int, int]]] | None,
        in_place: bool,
    ) -> ApplyReport:
        """Apply already-canonicalised ``edits`` against ``base`` and diff it.

        Nothing reaches the filesystem unless every edit landed, so the diff
        of a failed patch is empty rather than being the half of it that
        happened to match.
        """
        sources = self._read(base, edits)
        result = _with_read_reasons(
            apply_edits(edits, sources.contents, intervals=intervals), sources.unreadable
        )

        if result.ok:
            _write_all(base, result.new_contents)

        return ApplyReport(
            diff=sandbox.diff(base),
            result=result,
            in_place=in_place,
            base="the working tree" if in_place else f"HEAD ({ctx.head_sha or 'unknown'})",
        )

    def _require_clean(self, ctx: RepoContext) -> None:
        """Refuse an in-place apply unless the working tree is provably clean.

        The count is the snapshot ``resolve_repo`` took when the call started,
        not a fresh read. A write landing in the window between the two can
        only widen the diff this call reports: every edit is still matched
        against the file as it stands, so a late write is never overwritten.
        """
        if ctx.dirty_count == 0:
            return
        if ctx.dirty_count is None:
            message = (
                "--in-place refused: this repository's dirty-file count could not be read, "
                "so a clean tree cannot be proven. Drop --in-place to apply in a worktree."
            )
            raise AgentlessError(message)
        message = (
            f"--in-place refused: {ctx.dirty_count} files are modified or untracked. "
            "Commit or stash them, or drop --in-place to apply in a scratch worktree."
        )
        raise AgentlessError(message)

    def _language_of(self, path: str) -> str | None:
        """Return the language a repository-relative path is written in."""
        return self._extractor.SUPPORTED_EXTENSIONS.get(Path(path).suffix)

    def _canonical(self, ctx: RepoContext, edits: Sequence[Edit]) -> tuple[Edit, ...]:
        """Refuse every path outside the root and rewrite the rest as relative.

        Containment is always checked against ``ctx.root`` -- the repository
        the caller named -- never against whichever directory the edits are
        about to be written into, because a path admitted for staying inside a
        temporary directory is a path that was not checked at all.

        The rewrite is what makes ``app.py`` and ``./app.py`` one file rather
        than two, and it is why every outcome names a repository-relative path
        instead of the string the patch happened to carry.
        """
        return tuple(
            replace(edit, path=contained_path(ctx.root, edit.path).relative_to(ctx.root).as_posix())
            for edit in edits
        )

    def _read(self, base: Path, edits: Sequence[Edit]) -> _Sources:
        """Read every file the canonicalised ``edits`` name, from ``base``."""
        contents: dict[str, str] = {}
        unreadable: dict[str, str] = {}

        for edit in edits:
            if edit.path in contents or edit.path in unreadable:
                continue

            target = contained_path(base, edit.path)
            if not target.is_file():
                unreadable[edit.path] = NO_SUCH_FILE
                continue

            read = _read_source(target)
            if read.text is None:
                unreadable[edit.path] = read.skipped or "unreadable"
                continue
            contents[edit.path] = read.text

        return _Sources(contents=contents, unreadable=unreadable)


def _read_source(target: Path) -> BoundedRead:
    """Read one file the write side may have to write back, or refuse it.

    :func:`~agentless_mcp.util.fslimits.read_bounded` decodes with
    ``errors="replace"`` deliberately -- a stray byte must not fail a
    repository scan, and the tag cache's content digest is taken over exactly
    that decode. The write side cannot use it as-is: the round trip is not
    lossless, so an edit anywhere in a file would rewrite every undecodable
    byte in it. The shared reader stays lossy; the decision is made here, by
    decoding the bytes strictly for the files a patch actually names.
    """
    read = read_bounded(target)
    if read.text is None or REPLACEMENT_CHAR not in read.text:
        return read

    try:
        target.read_bytes().decode("utf-8")
    except UnicodeDecodeError:
        return BoundedRead(path=target, text=None, skipped=NOT_UTF8)
    except OSError as exc:
        return BoundedRead(path=target, text=None, skipped=f"unreadable: {_reason(exc)}")
    return read


def _with_read_reasons(result: ApplyResult, unreadable: Mapping[str, str]) -> ApplyResult:
    """Give every edit to a file that could not be read the reason it was not.

    ``apply_edits`` sees only the contents it was handed, so a file skipped
    for size, for permissions or for not being UTF-8 comes back as "no such
    file in this patch's scope" -- false about the caller's own repository,
    and the one diagnosis whose obvious next move (recreate the file) is
    destructive. ``check`` reports these per file; this is the same fact on
    the outcome that names the block.
    """
    if not unreadable:
        return result
    return replace(
        result,
        outcomes=tuple(
            _unread_outcome(outcome, unreadable[outcome.edit.path])
            if outcome.edit.path in unreadable
            else outcome
            for outcome in result.outcomes
        ),
    )


def _unread_outcome(outcome: EditOutcome, reason: str) -> EditOutcome:
    """Restate one outcome as the reason its file was never read."""
    status = EditStatus.NO_SUCH_FILE if reason == NO_SUCH_FILE else EditStatus.UNREADABLE
    return replace(outcome, status=status, reason=f"{outcome.edit.path}: {reason}")


def _write_all(base: Path, new_contents: Mapping[str, str]) -> None:
    """Write every edited file, or leave the tree exactly as it was.

    Three phases. Each new content is written through an exclusively-created,
    unpredictable sibling first. Backup names are then reserved the same way.
    Finally each original is moved to its backup before its staging file is
    moved into place. A later replacement failure restores completed targets
    from those backups in reverse order.

    Bytes are written rather than text because text-mode newline handling
    rewrites line endings on Windows. The target's mode is applied to the open
    staging descriptor where the platform supports it, so an executable script
    does not come back unexecutable and no path is reopened for the write.
    """
    staged: list[tuple[Path, Path]] = []
    for path, content in sorted(new_contents.items()):
        target = contained_path(base, path)
        try:
            staging = _stage_file(target, content)
        except OSError as exc:
            _discard(path for path, _ in staged)
            message = f"patch not applied: cannot write {path}: {_reason(exc)}; nothing was changed"
            raise AgentlessError(message) from exc
        staged.append((staging, target))

    backups: list[tuple[Path, Path]] = []
    try:
        for _, target in staged:
            backups.append((_reserve_sibling(target, BACKUP_SUFFIX), target))
    except OSError as exc:
        _discard(path for path, _ in staged)
        _discard(path for path, _ in backups)
        message = (
            f"patch not applied: cannot reserve rollback files: {_reason(exc)}; nothing changed"
        )
        raise AgentlessError(message) from exc

    completed: list[tuple[Path, Path]] = []
    for position, ((staging, target), (backup, _)) in enumerate(zip(staged, backups, strict=True)):
        try:
            target.replace(backup)
            completed.append((backup, target))
            staging.replace(target)
        except OSError as exc:
            rollback_failures = _rollback(completed)
            _discard(path for path, _ in staged[position:])
            completed_backups = {path for path, _ in completed}
            _discard(path for path, _ in backups if path not in completed_backups)
            if rollback_failures:
                failed = ", ".join(str(path) for path in rollback_failures)
                message = (
                    f"patch partly applied: cannot replace {target.name}: {_reason(exc)}; "
                    f"rollback failed; originals retained at: {failed}"
                )
            else:
                message = (
                    f"patch not applied: cannot replace {target.name}: {_reason(exc)}; "
                    "every original was restored"
                )
            raise AgentlessError(message) from exc

    _discard(path for path, _ in backups)


def _stage_file(target: Path, content: str) -> Path:
    """Write ``content`` to a new sibling owned by this invocation."""
    encoded = content.encode("utf-8")
    descriptor, raw_path = tempfile.mkstemp(
        prefix=f".{target.name}.", suffix=STAGING_SUFFIX, dir=target.parent
    )
    staging = Path(raw_path)
    try:
        mode = stat.S_IMODE(target.stat().st_mode)
        fchmod = getattr(os, "fchmod", None)
        if fchmod is not None:
            fchmod(descriptor, mode)

        _write_descriptor(descriptor, encoded, target.name)

        if fchmod is None:
            os.close(descriptor)
            descriptor = -1
            shutil.copymode(target, staging)
        else:
            os.close(descriptor)
            descriptor = -1
    except BaseException:
        if descriptor >= 0:
            with suppress(OSError):
                os.close(descriptor)
        with suppress(OSError):
            staging.unlink(missing_ok=True)
        raise

    return staging


def _write_descriptor(descriptor: int, content: bytes, target_name: str) -> None:
    """Write all bytes to an already-owned descriptor or raise."""
    remaining = memoryview(content)
    while remaining:
        written = os.write(descriptor, remaining)
        if written == 0:
            message = f"write returned zero bytes for {target_name}"
            raise OSError(message)
        remaining = remaining[written:]


def _reserve_sibling(target: Path, suffix: str) -> Path:
    """Atomically reserve and return one unpredictable sibling path."""
    descriptor, raw_path = tempfile.mkstemp(
        prefix=f".{target.name}.", suffix=suffix, dir=target.parent
    )
    os.close(descriptor)
    return Path(raw_path)


def _rollback(completed: Sequence[tuple[Path, Path]]) -> list[Path]:
    """Restore completed replacements and return backups that could not move."""
    failures: list[Path] = []
    for backup, target in reversed(completed):
        try:
            backup.replace(target)
        except OSError:
            failures.append(backup)
    return failures


def _discard(paths: Iterable[Path]) -> None:
    """Remove temporary files that will never be moved into place."""
    for path in paths:
        # A staging file that cannot be removed is litter; the failure that
        # brought us here is the one the caller needs to see.
        with suppress(OSError):
            path.unlink(missing_ok=True)


def _reason(exc: OSError) -> str:
    """Return the readable half of an OS error, however it was raised."""
    return exc.strerror or str(exc)
