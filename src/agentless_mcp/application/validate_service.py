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

**The commands come from the invocation, and a command out of the repository
is opt-in.** Nothing here looks a command up: there is no config file lookup,
no ``Makefile`` sniffing, no ``package.json`` scripts in this module. The
adapter may read one out of the analysed repository's ``.agentless-mcp.json``,
and when it does it must say so on the request -- ``test_cmd_from_repo``
with ``allow_repo_test_cmd`` -- because the repository is the thing being
judged, and letting it nominate its own judge is the injection path this
package is shaped around. Without the opt-in the run is refused here, before
anything is executed, rather than mitigated by a note the caller may not read.
The opt-in is the command-provenance enforcement. Separately, every command
runs with an allowlisted environment; ``passthrough_env`` names any additional
parent variables the caller deliberately exposes.

**A run bounds its own total cost.** ``timeout`` bounds one command;
``run_timeout``, when given, bounds the run. A batch is
``repeat_baseline + 1 + candidates x 2`` commands, so per-command bounds
multiply into hours. Past the deadline the remaining candidates are reported
``not_evaluated`` rather than skipped silently -- an unevaluated candidate and
a failed one are different answers.

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
from typing import Any, TypeVar

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

# One baseline run by default: repeating it costs a full test run per repeat
# and buys nothing on a suite that is not flaky. Raising it is how a caller
# who suspects flakiness finds out before the candidates inherit the doubt.
DEFAULT_REPEAT_BASELINE = 1

RECORD_KEY = "record"
RECORD_RUN = "run"
RECORD_CANDIDATE = "candidate"

# The enums the verdicts reader coerces through: every field that names a
# member of one is read as that member or refused.
_Member = TypeVar("_Member", bound=Enum)


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
    """Whether a candidate's edits landed in its worktree.

    ``not_evaluated`` is not ``failed``, for the same reason
    :class:`Verdict` draws that line: nothing was attempted, so reporting a
    failure would invent a result the run never produced. It is what a
    candidate gets when the run stopped before reaching it -- an unverified
    baseline, or a deadline that expired first.
    """

    OK = "ok"
    FAILED = "failed"
    NOT_EVALUATED = "not_evaluated"

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

    @property
    def measured(self) -> bool:
        """True when the command ran and this verdict is what it produced.

        ``error`` is the spawn failure -- the command never started, so a
        report that treats it as a failing suite is describing a measurement
        nobody took. ``timeout`` is on the other side of that line: the
        command ran, and outliving its bound is a result. This is the property
        the vote ladder excludes on, so it lives with the values rather than
        being re-derived by every reader.
        """
        return self not in (Verdict.ERROR, Verdict.NOT_EVALUATED)

    @classmethod
    def of(cls, result: RunResult) -> "Verdict":
        """Map a run result onto its verdict."""
        return cls(result.status.value)


@dataclass(frozen=True)
class ValidateRequest:
    """One validation run's inputs, all of them from the invocation.

    ``test_cmd_from_repo`` is the adapter declaring that ``test_cmd`` was
    read out of the repository under analysis rather than typed by the caller;
    ``allow_repo_test_cmd`` is the caller accepting that. The flag keys on
    where the command came from, not on which adapter asked, because that is
    the property the refusal protects.

    ``run_timeout`` bounds the whole run in seconds. ``None`` is the historical
    behaviour: no aggregate bound at all.
    """

    candidates: Path
    test_cmd: str
    repro_cmd: str | None = None
    timeout: int = DEFAULT_TIMEOUT_SECONDS
    jobs: int = DEFAULT_JOBS
    repeat_baseline: int = DEFAULT_REPEAT_BASELINE
    run_timeout: int | None = None
    passthrough_env: tuple[str, ...] = ()
    test_cmd_from_repo: bool = False
    allow_repo_test_cmd: bool = False


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
            measured=self.regression.measured,
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
    repeat_baseline: int = DEFAULT_REPEAT_BASELINE
    baseline_failures: int = 0

    @property
    def repro_valid(self) -> bool:
        """True only when the reproduction command failed on unpatched HEAD."""
        return self.repro_verdict is ReproVerdict.REPRODUCES

    @property
    def flaky_baseline(self) -> bool:
        """True when the baseline runs disagreed with each other."""
        return 0 < self.baseline_failures < self.repeat_baseline

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
            "repeat_baseline": self.repeat_baseline,
            "baseline_failures": self.baseline_failures,
            "flaky_baseline": self.flaky_baseline,
            "repro_verdict": self.repro_verdict.value,
            "repro_valid": self.repro_valid,
            "baseline_run": None if self.baseline_run is None else self.baseline_run.as_dict(),
            "repro_baseline_run": (
                None if self.repro_baseline_run is None else self.repro_baseline_run.as_dict()
            ),
        }


@dataclass(frozen=True)
class ValidateReport:
    """One validation run: the header, and a verdict per candidate.

    ``deadline_expired`` is carried rather than inferred from the verdicts: a
    candidate the deadline stopped and a candidate an unverified baseline
    stopped both read ``not_evaluated``, and only the run knows which happened.
    """

    header: RunHeader
    verdicts: tuple[CandidateVerdict, ...]
    deadline_expired: bool = False

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

    @property
    def never_measured(self) -> tuple[str, ...]:
        """The ids of candidates that applied and whose test command never ran.

        Not the same set as "failed the suite": these are the candidates whose
        regression command could not be started at all -- a spawn failure, a
        patch that renamed the runner -- so there is no measurement to report
        either way. They are excluded from the vote ladder for the same reason.
        """
        return tuple(
            verdict.id
            for verdict in self.verdicts
            if verdict.apply_status is ApplyStatus.OK and not verdict.regression.measured
        )

    def warnings(self) -> tuple[str, ...]:
        """Return the things a caller must not miss, for stderr.

        Each of these is a case where a report full of plausible-looking
        verdicts means less than it appears to, so they are said out loud on
        the error channel rather than left to be noticed in a JSON field.
        """
        lines: list[str] = []
        if self.header.flaky_baseline:
            lines.append(
                f"UNVERIFIED: {self.header.baseline_detail}. No candidate was evaluated -- "
                "a baseline that answers differently on identical input cannot tell a "
                "regression your patch caused from its own noise. Find the flaky test and "
                "exclude it, or narrow the command to a subset that is deterministic."
            )
        elif self.header.baseline is BaselineStatus.UNVERIFIED:
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
        unmeasured = self.never_measured
        if unmeasured:
            lines.append(
                f"NOTHING WAS MEASURED for {len(unmeasured)} of {len(self.verdicts)} candidates "
                f"({', '.join(unmeasured)}): the test command could not be started in their "
                "worktrees, although it started on the baseline. Their patches applied and "
                "nothing was run against them, so they are excluded from the vote ladder "
                "rather than reported as breaking the suite."
            )
        if self.deadline_expired:
            lines.append(
                "DEADLINE: the run's overall time budget expired before every candidate was "
                "evaluated. The candidates it did not reach are reported not_evaluated -- "
                "raise the budget, or narrow the candidate set."
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
        """Validate every candidate in ``request.candidates`` against ``ctx``.

        Refuses before anything is executed when the command came out of the
        repository under analysis and the caller did not opt in, and when the
        run's own bound is not a positive number of seconds.
        """
        _refuse_unowned_command(request)
        deadline = _deadline(request)

        candidates = load_candidates(request.candidates)
        header = self._baseline(ctx, request, count=len(candidates))

        if header.baseline is BaselineStatus.UNVERIFIED:
            return ValidateReport(
                header=header,
                verdicts=tuple(_not_evaluated(candidate) for candidate in candidates),
            )

        verdicts = self._evaluate_all(
            ctx, request, candidates, repro_valid=header.repro_valid, deadline=deadline
        )
        return ValidateReport(
            header=header,
            verdicts=verdicts,
            deadline_expired=any(
                verdict.apply_status is ApplyStatus.NOT_EVALUATED for verdict in verdicts
            ),
        )

    # ------------------------------------------------------------------
    # Baseline
    # ------------------------------------------------------------------

    def _baseline(self, ctx: RepoContext, request: ValidateRequest, *, count: int) -> RunHeader:
        """Run the test command, and the reproduction command, on unpatched HEAD.

        With ``repeat_baseline`` above one the test command runs that many
        times in that many fresh worktrees, and **any disagreement between
        those runs is UNVERIFIED**. A suite that passes twice and fails once
        cannot tell a regression from its own flakiness, so every candidate
        verdict computed against it would be a coin flip wearing a verdict's
        clothes. Reporting the disagreement is the only honest outcome, and
        the count is named so the caller can see how bad it is.
        """
        repeats = max(1, request.repeat_baseline)
        runs: list[RunResult] = []
        for _ in range(repeats):
            with sandbox.worktree(ctx.root) as tree:
                runs.append(
                    sandbox.run_command(
                        tree,
                        request.test_cmd,
                        timeout=request.timeout,
                        passthrough_env=request.passthrough_env,
                    )
                )

        failures = [run for run in runs if not run.passed]
        outcome = _baseline_outcome(runs, failures, repeats)
        if outcome.status is BaselineStatus.UNVERIFIED:
            return _header(ctx, request, count, outcome, repro_run=None)

        repro_run = None
        if request.repro_cmd is not None:
            with sandbox.worktree(ctx.root) as tree:
                repro_run = sandbox.run_command(
                    tree,
                    request.repro_cmd,
                    timeout=request.timeout,
                    passthrough_env=request.passthrough_env,
                )

        return _header(ctx, request, count, outcome, repro_run=repro_run)

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
        deadline: float | None,
    ) -> tuple[CandidateVerdict, ...]:
        """Evaluate every candidate, in parallel when asked, and sort the answers.

        The sort is what makes ``--jobs 2`` and ``--jobs 1`` produce the same
        document: worker completion order is not an input to the report, and
        a verdict file whose line order depends on scheduling would defeat
        every diff a caller wants to take against a previous run.

        The deadline is checked before a candidate starts rather than while it
        runs: a run in flight already has ``timeout`` bounding it, and killing
        a test suite half way through would produce a verdict nobody could act
        on. What the budget buys is that the *next* one does not start.
        """

        def evaluate(candidate: Candidate) -> CandidateVerdict:
            if deadline is not None and time.monotonic() >= deadline:
                return _deadline_expired(candidate, request.run_timeout)
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

            regression_run = sandbox.run_command(
                tree,
                request.test_cmd,
                timeout=request.timeout,
                passthrough_env=request.passthrough_env,
            )
            reproduction_run = (
                sandbox.run_command(
                    tree,
                    request.repro_cmd,
                    timeout=request.timeout,
                    passthrough_env=request.passthrough_env,
                )
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
    vote depends on must be present and of the right type, and every field
    that names an enum member is read through that enum: a document whose
    ``repro_valid`` went missing must not read as ``False`` and quietly demote
    the ladder by one rung, and neither must a ``regression`` a rename or a
    version skew spelled differently. A value this reader does not recognise
    is a refusal naming the line, never a candidate silently reduced to a loss.
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
        baseline=_enum(header, "baseline", 0, BaselineStatus),
        repro_valid=_boolean(header, "repro_valid", 0),
        test_cmd=_string(header, "test_cmd", 0),
        repro_cmd=_optional_string(header, "repro_cmd", 0),
        candidates=candidates,
    )


def _vote_candidate(record: dict[str, Any], position: int) -> VoteCandidate:
    """Turn one candidate record into the value the vote ranks.

    The three fields that decide a candidate's rung -- the apply status and
    the two verdicts -- are read through their enums rather than compared to a
    literal. A spelling this build does not know is a document it cannot rank,
    and the ladder quietly falling a rung is exactly the failure the strictness
    exists to prevent.
    """
    apply_field = record.get("apply")
    if not isinstance(apply_field, dict):
        message = f"line {position + 1}: candidate record has no 'apply' object"
        raise AtlasError(message)

    key = record.get("equivalence_key")
    if key is not None and not isinstance(key, str):
        message = f"line {position + 1}: 'equivalence_key' must be a string or null"
        raise AtlasError(message)

    regression = _enum(record, "regression", position, Verdict)
    reproduction = _optional_enum(record, "reproduction", position, Verdict)

    return VoteCandidate(
        id=_string(record, "id", position),
        index=_integer(record, "index", position),
        applied=_enum(apply_field, "status", position, ApplyStatus) is ApplyStatus.OK,
        measured=regression.measured,
        equivalence_key=key,
        regression_passed=regression is Verdict.PASSED,
        reproduction_passed=reproduction is Verdict.PASSED,
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


def _enum(record: dict[str, Any], field: str, position: int, kind: type[_Member]) -> _Member:
    """Read a required string field and coerce it through ``kind``, or refuse it.

    The refusal is an :class:`AtlasError` naming the line, the value and what
    was allowed, because every other refusal in this reader is: a bare
    ``ValueError`` out of an enum constructor reaches the caller as a
    traceback rather than as "your file is wrong at line 1".
    """
    value = _string(record, field, position)
    try:
        return kind(value)
    except ValueError as exc:
        allowed = ", ".join(member.value for member in kind)
        message = f"line {position + 1}: {field!r} is {value!r}, which is not one of: {allowed}"
        raise AtlasError(message) from exc


def _optional_enum(
    record: dict[str, Any], field: str, position: int, kind: type[_Member]
) -> _Member | None:
    """Read a field that is an enum member's value or explicitly null."""
    if record.get(field, ...) is None:
        return None
    return _enum(record, field, position, kind)


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


def _refuse_unowned_command(request: ValidateRequest) -> None:
    """Refuse a test command that the analysed repository named, unless opted in.

    The guard keys on where the command came from rather than on which caller
    is asking: an agent driving the CLI and a human at a terminal reach the
    same code, and only one of them reads a printed note.
    """
    if not request.test_cmd_from_repo or request.allow_repo_test_cmd:
        return
    message = (
        f"refusing to run {request.test_cmd!r}: it came from the repository under analysis, "
        "not from the invocation, so the repository would be choosing the command that judges "
        "it. Pass the command explicitly, or opt in with --allow-repo-test-cmd."
    )
    raise AtlasError(message)


def _deadline(request: ValidateRequest) -> float | None:
    """Return the monotonic instant the run must stop starting work at."""
    if request.run_timeout is None:
        return None
    if request.run_timeout <= 0:
        message = f"the run timeout must be a positive number of seconds, not {request.run_timeout}"
        raise AtlasError(message)
    return time.monotonic() + request.run_timeout


@dataclass(frozen=True)
class _BaselineOutcome:
    """What the baseline runs, taken together, established."""

    status: BaselineStatus
    detail: str
    representative: RunResult
    repeats: int
    failures: int


def _baseline_outcome(
    runs: Sequence[RunResult], failures: Sequence[RunResult], repeats: int
) -> _BaselineOutcome:
    """Decide what a set of baseline runs means.

    Three outcomes and no fourth: they all passed, they all failed, or they
    disagreed. The last is the reason this function exists -- a mixed result
    is not "mostly green", it is an instrument that gives a different reading
    each time it is used, and it invalidates the run exactly as a red baseline
    does.
    """
    if not failures:
        detail = (
            "the test command passed on unpatched HEAD"
            if repeats == 1
            else f"the test command passed on unpatched HEAD in all {repeats} baseline runs"
        )
        return _BaselineOutcome(BaselineStatus.OK, detail, runs[-1], repeats, 0)

    if len(failures) < repeats:
        detail = (
            f"FLAKY BASELINE: the test command disagreed across {repeats} runs on unpatched "
            f"HEAD -- {len(failures)} failed and {repeats - len(failures)} passed, with nothing "
            "changing between them"
        )
        return _BaselineOutcome(
            BaselineStatus.UNVERIFIED, detail, failures[0], repeats, len(failures)
        )

    return _BaselineOutcome(
        BaselineStatus.UNVERIFIED,
        _baseline_detail(failures[0]),
        failures[0],
        repeats,
        len(failures),
    )


def _header(
    ctx: RepoContext,
    request: ValidateRequest,
    count: int,
    outcome: _BaselineOutcome,
    *,
    repro_run: RunResult | None,
) -> RunHeader:
    """Build the run record from the baseline outcome and the request."""
    if outcome.status is BaselineStatus.UNVERIFIED:
        repro_verdict = (
            ReproVerdict.NOT_GIVEN if request.repro_cmd is None else ReproVerdict.NOT_EVALUATED
        )
    else:
        repro_verdict = _repro_verdict(repro_run)

    return RunHeader(
        receipt=envelope.receipt_fields(ctx),
        test_cmd=request.test_cmd,
        repro_cmd=request.repro_cmd,
        timeout=request.timeout,
        jobs=request.jobs,
        candidates=count,
        baseline=outcome.status,
        baseline_detail=outcome.detail,
        repro_verdict=repro_verdict,
        baseline_run=outcome.representative,
        repro_baseline_run=repro_run,
        repeat_baseline=outcome.repeats,
        baseline_failures=outcome.failures,
    )


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
        apply_status=ApplyStatus.NOT_EVALUATED,
        apply_reasons=("not evaluated: the baseline test run was UNVERIFIED",),
        equivalence_key=None,
        regression=Verdict.NOT_EVALUATED,
        reproduction=None,
        duration=0.0,
    )


def _deadline_expired(candidate: Candidate, budget: int | None) -> CandidateVerdict:
    """Return the verdict of a candidate the run's own budget did not reach."""
    return CandidateVerdict(
        id=candidate.id,
        index=candidate.index,
        apply_status=ApplyStatus.NOT_EVALUATED,
        apply_reasons=(f"not evaluated: the run's {budget}s budget expired before this candidate",),
        equivalence_key=None,
        regression=Verdict.NOT_EVALUATED,
        reproduction=None,
        duration=0.0,
    )
