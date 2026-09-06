"""Why a span exists: the commits that touched one symbol's lines, bodies included."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any, Protocol

from agentless_mcp.application.render import ROW_INDENT
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.symbol_service import SymbolSpan, resolve_symbol_span
from agentless_mcp.core import githistory, gitinfo
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.projectconfig import MAX_BUDGET, MIN_BUDGET
from agentless_mcp.core.symbols import symbol_stable_id
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util import bounds
from agentless_mcp.util.budget import TRUNCATION_MARKER_TOKENS, allocate
from agentless_mcp.util.errors import OperationFailed
from agentless_mcp.util.textsafe import one_line
from agentless_mcp.util.tokens import TokenCounter

DEFAULT_HISTORY_LIMIT = 10
# Under the envelope's 16k ceiling by the same margin expand keeps for the
# receipt, so this budget binds before the ceiling drops whole commits.
HISTORY_BUDGET_TOKENS = 12_000

# The budget cuts bodies and cannot cut a header row, so without a seat cap the
# row count alone decides the size: 500 rows measure 15.8k tokens, 120 measure 3.8k.
HISTORY_MAX_SEATS = 120

# A measured worst-case row is 32 tokens; the rest is room for the span header
# and the footer markers, so a caller's budget seats what it can really render.
HISTORY_TOKENS_PER_SEAT = 40

# --quiet prints nothing, so this bounds a misbehaving git and nothing else.
_DIRTY_CHECK_OUTPUT_BYTES = 4_096
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
        """Return the abbreviated sha every row and marker cites this commit by."""
        return self.sha[: gitinfo.SHORT_SHA_LENGTH]

    @property
    def body_shown(self) -> int:
        """Return how many body lines survived the budget."""
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
    # None is "git could not say", which a reader must not read as clean.
    dirty: bool | None
    seats_capped: bool = False
    dirty_note: str = ""

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
            "seats_capped": self.seats_capped,
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

        seats = min(limit, HISTORY_MAX_SEATS, max(1, budget // HISTORY_TOKENS_PER_SEAT))
        arguments = githistory.log_arguments(
            span.path, span.start_line, span.end_line, max_count=seats + 1
        )
        outcome = self._runner(
            ctx.root,
            arguments,
            timeout=githistory.HISTORY_TIMEOUT_SECONDS,
            max_output_bytes=githistory.MAX_HISTORY_OUTPUT_BYTES,
        )
        if outcome.text is None:
            raise OperationFailed(_failure(outcome.note, span))
        records = githistory.parse_log(outcome.text, capped=outcome.truncated)
        if not records:
            raise OperationFailed(_empty(span, capped=outcome.truncated))

        more = len(records) > seats
        dirty, dirty_note = self._dirty(ctx.root, span.path)
        return HistoryResult(
            target=symbol_stable_id(span.symbol),
            path=span.path,
            start_line=span.start_line,
            end_line=span.end_line,
            entries=self._fit([_entry(record) for record in records[:seats]], budget),
            more_commits=more,
            output_capped=outcome.truncated,
            dirty=dirty,
            seats_capped=more and seats < limit,
            dirty_note=dirty_note,
        )

    def _dirty(self, root: Path, path: str) -> tuple[bool | None, str]:
        outcome = self._runner(
            root,
            githistory.diff_arguments(path),
            timeout=gitinfo.GIT_TIMEOUT_SECONDS,
            max_output_bytes=_DIRTY_CHECK_OUTPUT_BYTES,
        )
        if outcome.returncode in (0, 1):
            return outcome.returncode == 1, ""
        return None, outcome.note

    def _fit(self, entries: list[HistoryEntry], budget: int) -> tuple[HistoryEntry, ...]:
        costs = [self._counter.count(_render_entry(entry)) for entry in entries]
        cut, share = allocate(costs, budget)
        return tuple(
            self._shorten(entry, share) if index in cut else entry
            for index, entry in enumerate(entries)
        )

    def _shorten(self, entry: HistoryEntry, share: int) -> HistoryEntry:
        if not entry.body_lines:
            return entry
        header = self._counter.count(_render_entry(replace(entry, body_lines=(), body_total=0)))
        room = share - header - TRUNCATION_MARKER_TOKENS
        lines = entry.body_lines
        # The header row always renders, so a body may go to nothing; the search
        # counts the rendered rows the settled cost was measured on.
        low, high = 0, len(lines)
        while low < high:
            middle = (low + high + 1) // 2
            if self._counter.count("\n".join(_body_rows(lines[:middle]))) <= room:
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
    if result.seats_capped:
        lines.append(
            MESSAGES.history_seats_capped.format(
                shown=count,
                path=one_line(result.path),
                start=result.start_line,
                end=result.end_line,
            )
        )
    elif result.more_commits:
        lines.append(MESSAGES.history_more_commits.format(shown=count))
    if result.dirty is None:
        lines.append(
            MESSAGES.history_dirty_unknown.format(
                path=one_line(result.path), note=one_line(result.dirty_note)
            )
        )
    elif result.dirty:
        lines.append(MESSAGES.history_dirty_file.format(path=one_line(result.path)))
    return "\n".join(lines) + "\n"


def _entry(record: githistory.CommitRecord) -> HistoryEntry:
    lines = record.body.splitlines()
    while lines and not lines[0].strip():
        lines.pop(0)
    while lines and not lines[-1].strip():
        lines.pop()
    return HistoryEntry(record.sha, record.authored, record.subject, tuple(lines), len(lines))


def _body_rows(lines: Sequence[str]) -> list[str]:
    return [f"{BODY_INDENT}{one_line(line)}" if line.strip() else "" for line in lines]


def _entry_lines(entry: HistoryEntry) -> list[str]:
    # Every row is indented and every field escaped per line, so a marker in
    # column 0 stays the one line repository text cannot forge.
    row = f"{one_line(entry.short_sha)}  {one_line(entry.authored)}  {one_line(entry.subject)}"
    lines = [f"{ROW_INDENT}{row}", *_body_rows(entry.body_lines)]
    if entry.body_shown < entry.body_total:
        # This marker names one commit, so it is a row and carries a row's
        # indent; only a whole-answer marker earns column 0.
        marker = MESSAGES.history_body_truncated.format(
            shown=entry.body_shown, total=entry.body_total, sha=one_line(entry.short_sha)
        )
        lines.append(f"{ROW_INDENT}{marker}")
    return lines


def _render_entry(entry: HistoryEntry) -> str:
    return "\n".join(_entry_lines(entry)) + "\n"


def _empty(span: SymbolSpan, *, capped: bool) -> str:
    if capped:
        return MESSAGES.history_output_capped_no_commits.format(
            bytes=githistory.MAX_HISTORY_OUTPUT_BYTES,
            path=span.path,
            start=span.start_line,
            end=span.end_line,
        )
    return MESSAGES.history_no_commits.format(
        path=span.path, start=span.start_line, end=span.end_line
    )


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
