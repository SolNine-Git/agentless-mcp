"""Why a span exists: the commits that touched one symbol's lines, bodies included."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, Protocol

from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import SymbolSpan, resolve_symbol_span
from agentless_mcp.core import githistory, gitinfo
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.projectconfig import MAX_BUDGET, MIN_BUDGET
from agentless_mcp.core.symbols import symbol_stable_id
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util import bounds
from agentless_mcp.util.errors import OperationFailed
from agentless_mcp.util.textsafe import one_line
from agentless_mcp.util.tokens import TokenCounter

DEFAULT_HISTORY_LIMIT = 10
# Under the envelope's 16k ceiling by the same margin expand keeps for the
# receipt, so this budget binds before the ceiling drops whole commits.
HISTORY_BUDGET_TOKENS = 12_000
_TRUNCATION_MARKER_TOKENS = 32
_DIRTY_CHECK_OUTPUT_BYTES = 4_096
ROW_INDENT = "  "
BODY_INDENT = "    "


class GitRunner(Protocol):
    """A bounded git invocation, injectable so the service is testable without git."""

    def __call__(
        self, cwd: Path, arguments: Sequence[str], *, timeout: float, max_output_bytes: int
    ) -> gitinfo.GitOutcome: ...


@dataclass(frozen=True)
class HistoryEntry:
    """One commit on the span, with the body lines the budget kept."""

    sha: str
    authored: str
    subject: str
    body_lines: tuple[str, ...]
    body_total: int

    @property
    def short_sha(self) -> str:
        return self.sha[: gitinfo.SHORT_SHA_LENGTH]

    @property
    def body_shown(self) -> int:
        return len(self.body_lines)

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this entry; a cut body carries its line counts."""
        record: dict[str, Any] = {
            "sha": self.sha,
            "authored": self.authored,
            "subject": self.subject,
            "body": "\n".join(self.body_lines),
        }
        if self.body_shown < self.body_total:
            record["body_truncated"] = {"lines_shown": self.body_shown, "lines": self.body_total}
        return record


@dataclass(frozen=True)
class HistoryResult:
    """The commits that touched one span, newest first, fitted to a budget."""

    target: str
    path: str
    start_line: int
    end_line: int
    entries: tuple[HistoryEntry, ...]
    more_commits: bool
    output_capped: bool
    dirty: bool

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form: the span, then the commits under ``commits``."""
        return {
            "target": self.target,
            "path": self.path,
            "start_line": self.start_line,
            "end_line": self.end_line,
            "commits": [entry.as_dict() for entry in self.entries],
            "more_commits": self.more_commits,
            "output_capped": self.output_capped,
            "dirty": self.dirty,
        }


class HistoryService:
    """Answers why a span exists from the commits that touched it. Holds no per-repository state."""

    def __init__(
        self,
        extractor: TreeSitterExtractor,
        counter: TokenCounter,
        runner: GitRunner = gitinfo.run_bounded,
    ) -> None:
        self._extractor = extractor
        self._counter = counter
        self._runner = runner

    def history(
        self,
        ctx: RepoContext,
        target: str,
        *,
        limit: int = DEFAULT_HISTORY_LIMIT,
        budget: int = HISTORY_BUDGET_TOKENS,
    ) -> HistoryResult:
        """Return the commits that touched the lines ``target`` spans, newest first."""
        bounds.within(limit, 1, bounds.MAX_LIMIT, "limit")
        bounds.within(budget, MIN_BUDGET, MAX_BUDGET, "budget")
        span, reason = resolve_symbol_span(ctx, self._extractor, target)
        if span is None:
            message = MESSAGES.history_target_unresolved.format(target=target, reason=reason)
            raise OperationFailed(message)
        if ctx.head_sha is None:
            raise OperationFailed(MESSAGES.history_no_git.format(note=ctx.note))

        arguments = githistory.log_arguments(
            span.path, span.start_line, span.end_line, max_count=limit + 1
        )
        outcome = self._runner(
            ctx.root,
            arguments,
            timeout=githistory.HISTORY_TIMEOUT_SECONDS,
            max_output_bytes=githistory.MAX_HISTORY_OUTPUT_BYTES,
        )
        if outcome.text is None:
            raise OperationFailed(_failure(outcome.note, span))
        records, _partial = githistory.parse_log(outcome.text)
        if not records:
            message = MESSAGES.history_no_commits.format(
                path=span.path, start=span.start_line, end=span.end_line
            )
            raise OperationFailed(message)

        entries = self._fit([_entry(record) for record in records[:limit]], budget)
        return HistoryResult(
            target=symbol_stable_id(span.symbol),
            path=span.path,
            start_line=span.start_line,
            end_line=span.end_line,
            entries=entries,
            more_commits=len(records) > limit,
            output_capped=outcome.truncated,
            dirty=self._dirty(ctx.root, span.path),
        )

    def _dirty(self, root: Path, path: str) -> bool:
        outcome = self._runner(
            root,
            githistory.diff_arguments(path),
            timeout=gitinfo.GIT_TIMEOUT_SECONDS,
            max_output_bytes=_DIRTY_CHECK_OUTPUT_BYTES,
        )
        return outcome.returncode == 1

    def _fit(self, entries: list[HistoryEntry], budget: int) -> tuple[HistoryEntry, ...]:
        # Water-filling, as expand does for cards: bodies that fit an equal
        # share stay whole and give their leftover back; the rest are cut alike.
        if not entries:
            return ()
        costs = {
            index: self._counter.count(_render_entry(entry)) for index, entry in enumerate(entries)
        }
        pending = set(costs)
        remaining = budget
        while pending:
            share = remaining // len(pending)
            settled = {index for index in pending if costs[index] <= share}
            if not settled:
                break
            remaining -= sum(costs[index] for index in settled)
            pending -= settled
        if not pending:
            return tuple(entries)
        share = max(0, remaining // len(pending))
        return tuple(
            self._shorten(entry, share) if index in pending else entry
            for index, entry in enumerate(entries)
        )

    def _shorten(self, entry: HistoryEntry, share: int) -> HistoryEntry:
        if not entry.body_lines:
            return entry
        header = self._counter.count(_render_entry(replace(entry, body_lines=(), body_total=0)))
        room = share - header - _TRUNCATION_MARKER_TOKENS
        lines = entry.body_lines
        low, high = 1, len(lines)
        while low < high:
            middle = (low + high + 1) // 2
            if self._counter.count("\n".join(lines[:middle])) <= room:
                low = middle
            else:
                high = middle - 1
        return replace(entry, body_lines=lines[:low])


def render_history(result: HistoryResult) -> str:
    """Render the span header, then one row per commit with its body lines indented."""
    count = len(result.entries)
    noun = "commit" if count == 1 else "commits"
    span = f"{one_line(result.path)}:{result.start_line}-{result.end_line}"
    lines = [f"{one_line(result.target)}  {span}  ({count} {noun}, newest first)"]
    for entry in result.entries:
        lines.extend(_entry_lines(entry))
    if result.output_capped:
        lines.append(
            MESSAGES.history_output_capped.format(
                bytes=githistory.MAX_HISTORY_OUTPUT_BYTES, count=count
            )
        )
    if result.more_commits:
        lines.append(MESSAGES.history_more_commits.format(shown=count))
    if result.dirty:
        lines.append(MESSAGES.history_dirty_file.format(path=one_line(result.path)))
    return "\n".join(lines) + "\n"


def _entry(record: githistory.CommitRecord) -> HistoryEntry:
    lines = record.body.splitlines()
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    return HistoryEntry(record.sha, record.authored, record.subject, tuple(lines), len(lines))


def _entry_lines(entry: HistoryEntry) -> list[str]:
    # Every row is indented and every field escaped per line, so a marker in
    # column 0 stays the one line repository text cannot forge.
    row = f"{one_line(entry.short_sha)}  {one_line(entry.authored)}  {one_line(entry.subject)}"
    lines = [f"{ROW_INDENT}{row}"]
    lines.extend(
        f"{BODY_INDENT}{one_line(line)}" if line.strip() else "" for line in entry.body_lines
    )
    if entry.body_shown < entry.body_total:
        lines.append(
            MESSAGES.history_body_truncated.format(
                shown=entry.body_shown, total=entry.body_total, sha=entry.short_sha
            )
        )
    return lines


def _render_entry(entry: HistoryEntry) -> str:
    return "\n".join(_entry_lines(entry)) + "\n"


def _failure(note: str, span: SymbolSpan) -> str:
    kind = githistory.classify_failure(note)
    if kind is githistory.HistoryFailure.NO_PATH:
        return MESSAGES.history_path_not_in_head.format(path=span.path)
    if kind is githistory.HistoryFailure.SPAN_BEYOND_HEAD:
        return MESSAGES.history_span_beyond_head.format(
            path=span.path, start=span.start_line, end=span.end_line
        )
    if kind is githistory.HistoryFailure.NO_GIT:
        return MESSAGES.history_no_git.format(note=note)
    return MESSAGES.history_git_failed.format(note=note)
