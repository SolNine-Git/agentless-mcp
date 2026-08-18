"""Candidate validation: baseline first, then one bounded run per candidate.

This is the Agentless validation phase recast model-free. The agent samples
the candidates; this decides, deterministically and with receipts, which of
them survive contact with the repository's own tests.

**The baseline is not optional and it comes first.** Before any candidate is
touched, the configured test command runs on unpatched HEAD in a fresh
worktree. If it does not pass there, the whole run is ``UNVERIFIED`` and every
candidate is reported ``not_evaluated``: a suite that was already red cannot
distinguish a patch that broke something from one that did not, and running
the candidates anyway would produce four verdicts that all mean "we do not
know" while looking exactly like verdicts that mean something.

**A reproduction test that passes on the baseline is a broken instrument.**
The point of a reproduction test is that it fails before the fix and passes
after -- the revert-test framing: a fix is pinned when reverting it makes the
test fail again. One that already passes proves nothing about any candidate,
so it is reported ``does_not_reproduce`` and its rung is removed from the vote
ladder rather than being counted as a pass everybody gets for free.

**Every candidate gets its own worktree.** Even at ``jobs=1``, because a
candidate that leaves a file behind must not be able to change the next
candidate's answer, and residue between runs is the classic way a validation
harness starts handing out a different result on every run. At ``jobs>1`` the
worktrees are genuinely concurrent; the git bookkeeping that creates and
removes them is serialised inside :mod:`agentless_mcp.core.sandbox`.

**The commands come only from the invocation.** Nothing here reads a test
command out of the repository under analysis -- no config file lookup, no
``Makefile`` sniffing, no ``package.json`` scripts. The repository is the
thing being judged; letting it nominate its own judge is the injection path
this whole package is shaped to avoid.

The output is one JSON Lines document: a ``run`` record carrying the receipt,
the commands and the baseline outcome, then a ``candidate`` record per
candidate. ``vote`` reads it back through :func:`load_verdicts`, which is the
only place that format is parsed and the only place it is written.
"""

import json
import time
from collections.abc import Sequence
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any

from agentless_mcp.application import envelope
from agentless_mcp.application.patch_service import PatchService, load_edits
from agentless_mcp.application.repo_context import RepoContext, resolve_repo
from agentless_mcp.core import sandbox
from agentless_mcp.core.patches import ApplyResult
from agentless_mcp.core.sandbox import RunResult, RunStatus
from agentless_mcp.core.vote import VoteCandidate
from agentless_mcp.util.errors import AtlasError

DEFAULT_TIMEOUT_SECONDS = 300
DEFAULT_JOBS = 1

RECORD_KEY = "record"
RECORD_RUN = "run"
RECORD_CANDIDATE = "candidate"


class BaselineStatus(str, Enum):
    """Whether the unpatched repository could answer the question at all."""

    OK = "ok"
    UNVERIFIED = "unverified"

    def __str__(self) -> str:
        """Return the member value, matching ``enum.StrEnum`` semantics."""
        return self.value


class ReproVerdict(str, Enum):
    """What the reproduction command did on the unpatched baseline.

    Only :data:`REPRODUCES` makes the reproduction rung of the vote ladder
    usable. The other four are each a different reason it is not, kept apart
    because "you gave us no reproduction test" and "the one you gave us
    already passes" call for different actions from the agent.
    """

    NOT_GIVEN = "not_given"
    REPRODUCES = "reproduces"
    DOES_NOT_REPRODUCE = "does_not_reproduce"
    UNRUNNABLE = "unrunnable"
    NOT_EVALUATED = "not_evaluated"

    def __str__(self) -> str:
        """Return the member value, matching ``enum.StrEnum`` semantics."""
        return self.value


class ApplyStatus(str, Enum):
    """Whether a candidate's edits landed in its worktree."""

    OK = "ok"
    FAILED = "failed"

    def __str__(self) -> str:
        """Return the member value, matching ``enum.StrEnum`` semantics."""
        return self.value


class Verdict(str, Enum):
    """One test command's outcome for one candidate.

    The four :class:`~agentless_mcp.core.sandbox.RunStatus` values plus
    ``not_evaluated``, which is what a candidate gets when the run never
    reached it -- an unverified baseline, or a patch that did not apply. It is
    deliberately not ``failed``: nothing was measured, and a report that says
    otherwise is inventing evidence.
    """

    PASSED = "passed"
    FAILED = "failed"
    TIMEOUT = "timeout"
    ERROR = "error"
    NOT_EVALUATED = "not_evaluated"

    def __str__(self) -> str:
        """Return the member value, matching ``enum.StrEnum`` semantics."""
        return self.value

    @classmethod
    def of(cls, result: RunResult) -> "Verdict":
        """Map a run result onto its verdict."""
        return cls(result.status.value)


@dataclass(frozen=True)
class ValidateRequest:
    """One validation run's inputs, all of them from the invocation."""

    candidates: Path
    test_cmd: str
    repro_cmd: str | None = None
    timeout: int = DEFAULT_TIMEOUT_SECONDS
    jobs: int = DEFAULT_JOBS


@dataclass(frozen=True)
class Candidate:
    """One candidate patch: its id, its first-appearance index and its text."""

    id: str
    index: int
    text: str


@dataclass(frozen=True)
class CandidateVerdict:
    """Everything one candidate produced, including why it produced nothing."""

    id: str
    index: int
    apply_status: ApplyStatus
    apply_reasons: tuple[str, ...]
    equivalence_key: str | None
    regression: Verdict
    reproduction: Verdict | None
    duration: float
    regression_run: RunResult | None = None
    reproduction_run: RunResult | None = None

    @property
    def usable(self) -> bool:
        """True when this candidate can take part in the vote at all."""
        return self.apply_status is ApplyStatus.OK and bool(self.equivalence_key)

    def as_vote_candidate(self) -> VoteCandidate:
        """Reduce this verdict to what the ladder and the cluster vote need."""
        return VoteCandidate(
            id=self.id,
            index=self.index,
            applied=self.apply_status is ApplyStatus.OK,
            equivalence_key=self.equivalence_key,
            regression_passed=self.regression is Verdict.PASSED,
            reproduction_passed=self.reproduction is Verdict.PASSED,
        )

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON Lines record for this candidate.

        Output tails ride along only for a run that did not pass. Carrying
        them for a green run would bury the interesting records under the
        console output of the boring ones.
        """
        record: dict[str, Any] = {
            RECORD_KEY: RECORD_CANDIDATE,
            "id": self.id,
            "index": self.index,
            "apply": {
                "status": self.apply_status.value,
                "reasons": list(self.apply_reasons),
            },
            "regression": self.regression.value,
            "reproduction": None if self.reproduction is None else self.reproduction.value,
            "equivalence_key": self.equivalence_key,
            "duration": self.duration,
        }
        tails = {
            name: run.as_dict()
            for name, run in (
                ("regression", self.regression_run),
                ("reproduction", self.reproduction_run),
            )
            if run is not None and not run.passed
        }
        if tails:
            record["tails"] = tails
        return record


@dataclass(frozen=True)
class RunHeader:
    """The run record: what was asked, what the baseline said, what it means."""

    receipt: dict[str, Any]
    test_cmd: str
    repro_cmd: str | None
    timeout: int
    jobs: int
    candidates: int
    baseline: BaselineStatus
    baseline_detail: str
    repro_verdict: ReproVerdict
    baseline_run: RunResult | None = None
    repro_baseline_run: RunResult | None = None

    @property
    def repro_valid(self) -> bool:
        """True only when the reproduction command failed on unpatched HEAD."""
        return self.repro_verdict is ReproVerdict.REPRODUCES

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON Lines record for this run."""
        return {
            RECORD_KEY: RECORD_RUN,
            "receipt": self.receipt,
            "test_cmd": self.test_cmd,
            "repro_cmd": self.repro_cmd,
            "timeout": self.timeout,
            "jobs": self.jobs,
            "candidates": self.candidates,
            "baseline": self.baseline.value,
            "baseline_detail": self.baseline_detail,
            "repro_verdict": self.repro_verdict.value,
            "repro_valid": self.repro_valid,
            "baseline_run": None if self.baseline_run is None else self.baseline_run.as_dict(),
            "repro_baseline_run": (
                None if self.repro_baseline_run is None else self.repro_baseline_run.as_dict()
            ),
        }


@dataclass(frozen=True)
class ValidateReport:
    """One validation run: the header, and a verdict per candidate."""

    header: RunHeader
    verdicts: tuple[CandidateVerdict, ...]

    @property
    def any_passed(self) -> bool:
        """True when at least one candidate reached a decisive ladder tier.

        Decisive means tier one or tier two: applied, has an equivalence key,
        and broke no regression test. The apply-ok fallback tier is explicitly
        not enough -- it is the tier that exists to say nothing worked.
        """
        return any(
            verdict.usable and verdict.regression is Verdict.PASSED for verdict in self.verdicts
        )

    def jsonl(self) -> str:
        """Return the whole run as a JSON Lines document."""
        records = [self.header.as_dict(), *(verdict.as_dict() for verdict in self.verdicts)]
        return "".join(json.dumps(record) + "\n" for record in records)

    def summary_line(self) -> str:
        """Return the one-line summary for the receipt on stderr."""
        passed = sum(1 for verdict in self.verdicts if verdict.regression is Verdict.PASSED)
        applied = sum(1 for verdict in self.verdicts if verdict.apply_status is ApplyStatus.OK)
        return (
            f"baseline {self.header.baseline.value}; {applied} of {len(self.verdicts)} "
            f"candidates applied, {passed} passed the regression suite"
        )

    def warnings(self) -> tuple[str, ...]:
        """Return the things a caller must not miss, for stderr.

        Both of these are cases where a report full of plausible-looking
        verdicts means less than it appears to, so they are said out loud on
        the error channel rather than left to be noticed in a JSON field.
        """
        lines: list[str] = []
        if self.header.baseline is BaselineStatus.UNVERIFIED:
            lines.append(
                "UNVERIFIED: the test command did not pass on unpatched HEAD "
                f"({self.header.baseline_detail}). No candidate was evaluated -- "
                "a red baseline cannot tell a regression from a pre-existing failure."
            )
        if self.header.repro_verdict is ReproVerdict.DOES_NOT_REPRODUCE:
            lines.append(
                "does_not_reproduce: the reproduction command PASSED on unpatched HEAD, "
                "so it does not reproduce the bug. Its results are meaningless and it was "
                "excluded from the vote ladder. Write a test that fails before the fix."
            )
        if self.header.repro_verdict is ReproVerdict.UNRUNNABLE:
            run = self.header.repro_baseline_run
            detail = run.stderr_tail.strip() if run is not None else "no detail"
            lines.append(
                "unrunnable: the reproduction command could not be started on unpatched HEAD "
                f"({detail}). It was excluded from the vote ladder."
            )
        return tuple(lines)


@dataclass(frozen=True)
class LoadedVerdicts:
    """A verdicts document read back from disk, parsed into typed values."""

    baseline: BaselineStatus
    repro_valid: bool
    test_cmd: str
    repro_cmd: str | None
    candidates: tuple[VoteCandidate, ...]


class ValidateService:
    """Run the baseline, then every candidate, and report what happened."""

    def __init__(self, patches: PatchService) -> None:
        self._patches = patches

    def validate(self, ctx: RepoContext, request: ValidateRequest) -> ValidateReport:
        """Validate every candidate in ``request.candidates`` against ``ctx``."""
        candidates = load_candidates(request.candidates)
        header = self._baseline(ctx, request, count=len(candidates))

        if header.baseline is BaselineStatus.UNVERIFIED:
            return ValidateReport(
                header=header,
                verdicts=tuple(_not_evaluated(candidate) for candidate in candidates),
            )

        verdicts = self._evaluate_all(ctx, request, candidates, repro_valid=header.repro_valid)
        return ValidateReport(header=header, verdicts=verdicts)

    # ------------------------------------------------------------------
    # Baseline
    # ------------------------------------------------------------------

    def _baseline(self, ctx: RepoContext, request: ValidateRequest, *, count: int) -> RunHeader:
        """Run the test command, and the reproduction command, on unpatched HEAD."""
        fields = envelope.receipt_fields(ctx)

        with sandbox.worktree(ctx.root) as tree:
            test_run = sandbox.run_command(tree, request.test_cmd, timeout=request.timeout)
            if not test_run.passed:
                return RunHeader(
                    receipt=fields,
                    test_cmd=request.test_cmd,
                    repro_cmd=request.repro_cmd,
                    timeout=request.timeout,
                    jobs=request.jobs,
                    candidates=count,
                    baseline=BaselineStatus.UNVERIFIED,
                    baseline_detail=_baseline_detail(test_run),
                    repro_verdict=(
                        ReproVerdict.NOT_GIVEN
                        if request.repro_cmd is None
                        else ReproVerdict.NOT_EVALUATED
                    ),
                    baseline_run=test_run,
                )

            repro_run = (
                None
                if request.repro_cmd is None
                else sandbox.run_command(tree, request.repro_cmd, timeout=request.timeout)
            )

        return RunHeader(
            receipt=fields,
            test_cmd=request.test_cmd,
            repro_cmd=request.repro_cmd,
            timeout=request.timeout,
            jobs=request.jobs,
            candidates=count,
            baseline=BaselineStatus.OK,
            baseline_detail="the test command passed on unpatched HEAD",
            repro_verdict=_repro_verdict(repro_run),
            baseline_run=test_run,
            repro_baseline_run=repro_run,
        )

    # ------------------------------------------------------------------
    # Candidates
    # ------------------------------------------------------------------

    def _evaluate_all(
        self,
        ctx: RepoContext,
        request: ValidateRequest,
        candidates: Sequence[Candidate],
        *,
        repro_valid: bool,
    ) -> tuple[CandidateVerdict, ...]:
        """Evaluate every candidate, in parallel when asked, and sort the answers.

        The sort is what makes ``--jobs 2`` and ``--jobs 1`` produce the same
        document: worker completion order is not an input to the report, and
        a verdict file whose line order depends on scheduling would defeat
        every diff a caller wants to take against a previous run.
        """

        def evaluate(candidate: Candidate) -> CandidateVerdict:
            return self._evaluate(ctx, request, candidate, repro_valid=repro_valid)

        if request.jobs > 1 and len(candidates) > 1:
            with ThreadPoolExecutor(max_workers=request.jobs) as pool:
                verdicts = list(pool.map(evaluate, candidates))
        else:
            verdicts = [evaluate(candidate) for candidate in candidates]

        return tuple(sorted(verdicts, key=lambda verdict: verdict.index))

    def _evaluate(
        self,
        ctx: RepoContext,
        request: ValidateRequest,
        candidate: Candidate,
        *,
        repro_valid: bool,
    ) -> CandidateVerdict:
        """Apply one candidate in its own worktree and run the tests there."""
        started = time.monotonic()
        with sandbox.worktree(ctx.root) as tree:
            # The worktree is the repository this candidate is judged in, so
            # it is the root every path in the patch is contained against. A
            # containment refusal here still aborts the whole run: a candidate
            # naming a path outside the repository is not a failed candidate,
            # it is a refused invocation.
            scoped = resolve_repo(tree, None)

            try:
                parsed = load_edits(candidate.text)
            except AtlasError as error:
                return _apply_failed(candidate, (str(error),), started)

            if parsed.errors:
                reasons = tuple(
                    f"block {error.index} ({error.path or 'no path'}): {error.reason}"
                    for error in parsed.errors
                )
                return _apply_failed(candidate, reasons, started)
            if not parsed.edits:
                return _apply_failed(candidate, ("the candidate contains no edits",), started)

            # Normalising first computes the equivalence key against the same
            # unpatched content the apply is about to match against, and
            # reports every block that would not land -- so a candidate that
            # cannot apply costs no test run at all.
            normalized = self._patches.normalize(parsed.edits, scoped)
            if not normalized.ok:
                return _apply_failed(candidate, _reasons(normalized.result), started)

            applied = self._patches.apply(parsed.edits, scoped, in_place=True)
            if not applied.ok:
                return _apply_failed(candidate, _reasons(applied.result), started)

            # A patch that applied and changed nothing has no equivalence
            # class to vote in: the Agentless ladder's `patch_key.strip()`
            # guard, kept in every tier.
            key = normalized.key if normalized.result.new_contents else None

            regression_run = sandbox.run_command(tree, request.test_cmd, timeout=request.timeout)
            reproduction_run = (
                sandbox.run_command(tree, request.repro_cmd, timeout=request.timeout)
                if repro_valid and request.repro_cmd is not None
                else None
            )

        return CandidateVerdict(
            id=candidate.id,
            index=candidate.index,
            apply_status=ApplyStatus.OK,
            apply_reasons=(),
            equivalence_key=key,
            regression=Verdict.of(regression_run),
            reproduction=None if reproduction_run is None else Verdict.of(reproduction_run),
            duration=round(time.monotonic() - started, 3),
            regression_run=regression_run,
            reproduction_run=reproduction_run,
        )


# ---------------------------------------------------------------------------
# Candidate loading
# ---------------------------------------------------------------------------


def load_candidates(directory: Path) -> tuple[Candidate, ...]:
    """Read every candidate file in ``directory``, in sorted order.

    One file is one candidate, its stem is its id, and the sorted order of the
    directory is first-appearance order -- the tiebreak the vote uses between
    equal-sized clusters, so it has to be a property of the input rather than
    of the filesystem's own ordering.

    Two files sharing a stem are refused rather than silently collapsed: the
    ids would collide in the report and one candidate's verdict would appear
    to be the other's.
    """
    resolved = directory.expanduser().resolve()
    if not resolved.is_dir():
        message = f"--candidates must name a directory; {resolved} is not one"
        raise AtlasError(message)

    files = sorted(entry for entry in resolved.iterdir() if entry.is_file())
    if not files:
        message = f"no candidate files in {resolved}: one file per candidate patch"
        raise AtlasError(message)

    seen: dict[str, Path] = {}
    candidates: list[Candidate] = []
    for index, path in enumerate(files):
        if path.stem in seen:
            message = (
                f"two candidates share the id {path.stem!r}: {seen[path.stem].name} and "
                f"{path.name}. Candidate ids come from the filename stem and must be unique."
            )
            raise AtlasError(message)
        seen[path.stem] = path

        try:
            text = path.read_text(encoding="utf-8")
        except OSError as exc:
            message = f"cannot read candidate {path.name}: {exc.strerror}"
            raise AtlasError(message) from exc
        except UnicodeDecodeError as exc:
            message = f"candidate {path.name} is not UTF-8 text"
            raise AtlasError(message) from exc

        candidates.append(Candidate(id=path.stem, index=index, text=text))

    return tuple(candidates)


# ---------------------------------------------------------------------------
# Verdict document parsing
# ---------------------------------------------------------------------------


def load_verdicts(text: str) -> LoadedVerdicts:
    """Parse a verdicts document back into typed values, or refuse it.

    Strict on purpose, and the only reader of this format. Every field the
    vote depends on must be present and of the right type: a document whose
    ``repro_valid`` went missing must not read as ``False`` and quietly demote
    the ladder by one rung.
    """
    lines = [line for line in text.splitlines() if line.strip()]
    if not lines:
        message = "the verdicts file is empty: run `agentless-mcp validate` to produce one"
        raise AtlasError(message)

    records = [_record(line, position) for position, line in enumerate(lines)]
    header = records[0]
    if _string(header, RECORD_KEY, 0) != RECORD_RUN:
        message = f"the first line of a verdicts file must be a '{RECORD_RUN}' record"
        raise AtlasError(message)

    candidates = tuple(
        _vote_candidate(record, position)
        for position, record in enumerate(records[1:], start=1)
        if _string(record, RECORD_KEY, position) == RECORD_CANDIDATE
    )

    return LoadedVerdicts(
        baseline=BaselineStatus(_string(header, "baseline", 0)),
        repro_valid=_boolean(header, "repro_valid", 0),
        test_cmd=_string(header, "test_cmd", 0),
        repro_cmd=_optional_string(header, "repro_cmd", 0),
        candidates=candidates,
    )


def _vote_candidate(record: dict[str, Any], position: int) -> VoteCandidate:
    """Turn one candidate record into the value the vote ranks."""
    apply_field = record.get("apply")
    if not isinstance(apply_field, dict):
        message = f"line {position + 1}: candidate record has no 'apply' object"
        raise AtlasError(message)

    key = record.get("equivalence_key")
    if key is not None and not isinstance(key, str):
        message = f"line {position + 1}: 'equivalence_key' must be a string or null"
        raise AtlasError(message)

    reproduction = record.get("reproduction")
    if reproduction is not None and not isinstance(reproduction, str):
        message = f"line {position + 1}: 'reproduction' must be a string or null"
        raise AtlasError(message)

    return VoteCandidate(
        id=_string(record, "id", position),
        index=_integer(record, "index", position),
        applied=_string(apply_field, "status", position) == ApplyStatus.OK.value,
        equivalence_key=key,
        regression_passed=_string(record, "regression", position) == Verdict.PASSED.value,
        reproduction_passed=reproduction == Verdict.PASSED.value,
    )


def _record(line: str, position: int) -> dict[str, Any]:
    """Parse one JSON Lines line into an object, or refuse it."""
    try:
        parsed = json.loads(line)
    except json.JSONDecodeError as exc:
        message = f"line {position + 1} of the verdicts file is not valid JSON: {exc}"
        raise AtlasError(message) from exc
    if not isinstance(parsed, dict):
        message = f"line {position + 1} of the verdicts file is not a JSON object"
        raise AtlasError(message)
    return parsed


def _string(record: dict[str, Any], field: str, position: int) -> str:
    """Read a required string field, or refuse the record."""
    value = record.get(field)
    if not isinstance(value, str):
        message = f"line {position + 1}: missing a string {field!r}"
        raise AtlasError(message)
    return value


def _optional_string(record: dict[str, Any], field: str, position: int) -> str | None:
    """Read a field that is a string or explicitly null."""
    value = record.get(field, ...)
    if value is None:
        return None
    if not isinstance(value, str):
        message = f"line {position + 1}: {field!r} must be a string or null"
        raise AtlasError(message)
    return value


def _boolean(record: dict[str, Any], field: str, position: int) -> bool:
    """Read a required boolean field, or refuse the record."""
    value = record.get(field)
    if not isinstance(value, bool):
        message = f"line {position + 1}: missing a boolean {field!r}"
        raise AtlasError(message)
    return value


def _integer(record: dict[str, Any], field: str, position: int) -> int:
    """Read a required integer field, or refuse the record."""
    value = record.get(field)
    if not isinstance(value, int) or isinstance(value, bool):
        message = f"line {position + 1}: missing an integer {field!r}"
        raise AtlasError(message)
    return value


# ---------------------------------------------------------------------------
# Small helpers
# ---------------------------------------------------------------------------


def _repro_verdict(run: RunResult | None) -> ReproVerdict:
    """Decide what the reproduction command's baseline outcome means.

    A failure or a timeout is the bug showing itself, which is what makes the
    test usable. A pass means it does not reproduce; a spawn error means there
    was no test to speak of.
    """
    if run is None:
        return ReproVerdict.NOT_GIVEN
    if run.status is RunStatus.PASSED:
        return ReproVerdict.DOES_NOT_REPRODUCE
    if run.status is RunStatus.ERROR:
        return ReproVerdict.UNRUNNABLE
    return ReproVerdict.REPRODUCES


def _baseline_detail(run: RunResult) -> str:
    """Explain a baseline that did not pass, in one line."""
    if run.status is RunStatus.TIMEOUT:
        return f"the test command timed out after {run.duration}s on unpatched HEAD"
    if run.status is RunStatus.ERROR:
        return f"the test command could not be started: {run.stderr_tail.strip()}"
    return f"the test command exited {run.exit_code} on unpatched HEAD"


def _reasons(result: ApplyResult) -> tuple[str, ...]:
    """Render every non-applied edit as one reason line."""
    return tuple(
        f"block {outcome.edit.index} ({outcome.edit.path}) {outcome.status.value}: {outcome.reason}"
        for outcome in result.failures
    )


def _apply_failed(candidate: Candidate, reasons: Sequence[str], started: float) -> CandidateVerdict:
    """Return the verdict of a candidate whose edits did not land."""
    return CandidateVerdict(
        id=candidate.id,
        index=candidate.index,
        apply_status=ApplyStatus.FAILED,
        apply_reasons=tuple(reasons),
        equivalence_key=None,
        regression=Verdict.NOT_EVALUATED,
        reproduction=None,
        duration=round(time.monotonic() - started, 3),
    )


def _not_evaluated(candidate: Candidate) -> CandidateVerdict:
    """Return the verdict every candidate gets when the baseline is unverified."""
    return CandidateVerdict(
        id=candidate.id,
        index=candidate.index,
        apply_status=ApplyStatus.FAILED,
        apply_reasons=("not evaluated: the baseline test run was UNVERIFIED",),
        equivalence_key=None,
        regression=Verdict.NOT_EVALUATED,
        reproduction=None,
        duration=0.0,
    )
