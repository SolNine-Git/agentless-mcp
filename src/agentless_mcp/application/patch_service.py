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
"""

import json
from collections.abc import Mapping, Sequence
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
    ParseResult,
    apply_edits,
    parse_blocks,
)
from agentless_mcp.util.errors import AtlasError
from agentless_mcp.util.fslimits import contained_path, read_bounded

EDITS_KEY = "edits"


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
        raise AtlasError(message) from exc

    if not isinstance(document, dict) or EDITS_KEY not in document:
        message = f"edits document must be a JSON object with an '{EDITS_KEY}' list"
        raise AtlasError(message)

    entries = document[EDITS_KEY]
    if not isinstance(entries, list):
        message = f"'{EDITS_KEY}' must be a list of edit objects"
        raise AtlasError(message)

    return ParseResult(
        edits=tuple(_edit_from(entry, position) for position, entry in enumerate(entries)),
        errors=(),
    )


def _edit_from(entry: object, position: int) -> Edit:
    """Turn one JSON edit object into an :class:`Edit`, or refuse it."""
    if not isinstance(entry, dict):
        message = f"edit {position} is not a JSON object"
        raise AtlasError(message)

    values: dict[str, str] = {}
    for field in ("path", "search", "replace"):
        value = entry.get(field)
        if not isinstance(value, str):
            message = f"edit {position} is missing a string '{field}'"
            raise AtlasError(message)
        values[field] = value

    index = entry.get("index", position)
    if not isinstance(index, int):
        message = f"edit {position} has a non-integer 'index'"
        raise AtlasError(message)

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
        """Apply already-canonicalised ``edits`` against ``base`` and diff it."""
        sources = self._read(base, edits)
        result = apply_edits(edits, sources.contents, intervals=intervals)

        for path, content in result.new_contents.items():
            contained_path(base, path).write_text(content, encoding="utf-8")

        return ApplyReport(
            diff=sandbox.diff(base),
            result=result,
            in_place=in_place,
            base="the working tree" if in_place else f"HEAD ({ctx.head_sha or 'unknown'})",
        )

    def _require_clean(self, ctx: RepoContext) -> None:
        """Refuse an in-place apply unless the working tree is provably clean."""
        if ctx.dirty_count == 0:
            return
        if ctx.dirty_count is None:
            message = (
                "--in-place refused: this repository's dirty-file count could not be read, "
                "so a clean tree cannot be proven. Drop --in-place to apply in a worktree."
            )
            raise AtlasError(message)
        message = (
            f"--in-place refused: {ctx.dirty_count} files are modified or untracked. "
            "Commit or stash them, or drop --in-place to apply in a scratch worktree."
        )
        raise AtlasError(message)

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
                unreadable[edit.path] = "no such file in this repository"
                continue

            read = read_bounded(target)
            if read.text is None:
                unreadable[edit.path] = read.skipped or "unreadable"
                continue
            contents[edit.path] = read.text

        return _Sources(contents=contents, unreadable=unreadable)
