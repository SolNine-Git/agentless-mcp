"""The patch linter: the mechanical half of a first-pass review, before tests.

:mod:`agentless_mcp.core.patchlint` owns the checks and knows nothing about
repositories; this owns the one thing it must not -- reading the repository the
patch is about. It scans the tree, reads the texts, reads the dependency
manifests and builds the Phase 6 resolver, hands all four over as one value,
and maps the findings into the view models the adapters render.

**CLI only, like validate and vote, and for the same reason.** A patch is
write-side input: it arrives as model output, it names files, and running
checks over it is a step a human asked for. No MCP tool reaches this service,
so nothing an analysed repository contains can provoke a lint run.

The scan happens once per call and is shared by every candidate, because the
repository the candidates are judged against is the same repository for all of
them -- and because scanning it per candidate would make a ten-candidate lint
ten scans of the same unchanged tree.

The pair checks in :mod:`agentless_mcp.core.erosion` compare each edited file's
whole text before and after the change. A run that selects only pair checks
reads the edited files and does not scan the repository.

Nothing here decides anything. The report has no verdict, ``lint`` exits 0
whatever it finds, and a candidate whose patch does not parse is reported with
its parse errors rather than skipped -- but it is not *checked*, because the
write side refuses a partly-parsed patch whole and findings about the blocks
that did parse would describe a patch that can never be applied.
"""

from collections.abc import Callable, Collection, Mapping, Sequence
from dataclasses import dataclass, replace
from enum import Enum
from pathlib import Path

from agentless_mcp.application import render
from agentless_mcp.application.patch_service import load_edits
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core import (
    cache,
    complexity,
    erosion,
    patchlint,
    projectconfig,
    refs,
    resolve,
    unidiff,
)
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.patches import BlockError, Edit, EditOutcome, ParseResult, apply_edits
from agentless_mcp.core.projectconfig import CONFIG_FILENAME, ProjectConfig
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.errors import (
    InputUnreadable,
    OperationFailed,
    SecurityRefusal,
)
from agentless_mcp.util.fslimits import (
    BoundedBytes,
    BoundedRead,
    contained_path,
    decoded,
    decoded_strict,
    file_stays_inside,
    read_bounded,
    read_bounded_bytes,
)

# What a manifest at the repository root is refused with when it is a symlink
# whose target is outside that root. Reported as an unread manifest rather than
# raised, so the report says which one contributed nothing and why.
MANIFEST_OUTSIDE_ROOT = "refused: a symlink whose target is outside the repository"


@dataclass(frozen=True)
class LintCandidateInput:
    """One patch to lint: an id for the report, and the edits it parsed to.

    The edits arrive parsed rather than as the text they came from, because
    there is more than one way in -- SEARCH/REPLACE blocks, an ``edits.json``,
    a unified diff -- and exactly one of them should be a parse. Handing the
    service raw text would put that decision below the boundary, where a second
    format would become a branch inside the checks instead of a loader beside
    them.

    ``notes`` is what the input held that has nothing to check: a binary file
    in a diff, a mode-only change. They are reported beside the findings, never
    instead of them, because a section that vanishes is a clean report about a
    patch nobody fully read.
    """

    id: str
    parsed: ParseResult
    notes: tuple[unidiff.DiffNote, ...] = ()


class CheckInput(str, Enum):
    """What a lint check reads: a parsed patch with the repository scan, or text pairs."""

    PATCH = "patch"
    PAIRS = "pairs"


PairRunner = Callable[[Sequence[erosion.TextPair], ProjectConfig], tuple[erosion.PairFinding, ...]]


@dataclass(frozen=True)
class LintCheck:
    """One lint check, the input it needs, and its runner when it is a pair check."""

    name: str
    needs: CheckInput
    run_pairs: PairRunner | None = None

    def __post_init__(self) -> None:
        if (self.needs is CheckInput.PAIRS) != (self.run_pairs is not None):
            message = f"{self.name}: a pair check needs a runner, and a patch check has none"
            raise ValueError(message)


def _complexity_delta(
    pairs: Sequence[erosion.TextPair], config: ProjectConfig
) -> tuple[erosion.PairFinding, ...]:
    threshold = projectconfig.resolve(
        None, config.complexity_threshold, complexity.DEFAULT_THRESHOLD
    )
    return erosion.complexity_delta(pairs, threshold)


def _contract_drift(
    pairs: Sequence[erosion.TextPair], config: ProjectConfig
) -> tuple[erosion.PairFinding, ...]:
    return erosion.contract_drift(pairs, config.boundary_globs)


CHECKS: tuple[LintCheck, ...] = (
    LintCheck(patchlint.CHECK_UNDECLARED_IMPORTS, CheckInput.PATCH),
    LintCheck(patchlint.CHECK_SHADOWING, CheckInput.PATCH),
    LintCheck(patchlint.CHECK_NEAR_DUPLICATES, CheckInput.PATCH),
    LintCheck(patchlint.CHECK_DANGLING_REFERENCES, CheckInput.PATCH),
    LintCheck(patchlint.CHECK_DANGLING_CALLERS, CheckInput.PATCH),
    LintCheck(patchlint.CHECK_ARITY, CheckInput.PATCH),
    LintCheck(patchlint.CHECK_CYCLE_DELTA, CheckInput.PATCH),
    LintCheck(erosion.CHECK_COMPLEXITY_DELTA, CheckInput.PAIRS, _complexity_delta),
    LintCheck(erosion.CHECK_CONTRACT_DRIFT, CheckInput.PAIRS, _contract_drift),
)
CHECK_NAMES = tuple(check.name for check in CHECKS)
PAIR_CHECK_NAMES = tuple(check.name for check in CHECKS if check.needs is CheckInput.PAIRS)


def unknown_check_refusal(names: Collection[str]) -> str:
    """Return why ``names`` is not a usable check selection, or an empty string."""
    if not names:
        return MESSAGES.lint_no_checks.format(checks=", ".join(CHECK_NAMES))
    unknown = [name for name in names if name not in CHECK_NAMES]
    if unknown:
        return MESSAGES.lint_unknown_check.format(name=unknown[0], checks=", ".join(CHECK_NAMES))
    return ""


def proposed_check_refusal(names: Collection[str]) -> str:
    """Return why the hook form cannot run ``names``, or an empty string."""
    patch = [name for name in names if name not in PAIR_CHECK_NAMES]
    if not patch:
        return ""
    return MESSAGES.lint_path_takes_pair_checks.format(
        pair_checks=", ".join(PAIR_CHECK_NAMES), checks=", ".join(patch)
    )


Judge = Callable[[LintCandidateInput], tuple[render.LintFinding, ...]]


class LintService:
    """Runs the deterministic lint checks for one repository."""

    def __init__(self, extractor: TreeSitterExtractor) -> None:
        self._extractor = extractor

    def lint(
        self,
        ctx: RepoContext,
        candidates: Sequence[LintCandidateInput],
        checks: Collection[str] = CHECK_NAMES,
    ) -> render.LintReportView:
        """Lint every candidate with ``checks``, scanning once and only for a patch check."""
        judge = self._judge(ctx, _select(checks))
        return render.LintReportView(
            candidates=tuple(
                render.LintCandidate(id=candidate.id, findings=judge(candidate))
                for candidate in candidates
            )
        )

    def lint_proposed(
        self,
        ctx: RepoContext,
        path: str,
        after: Path,
        checks: Collection[str] = PAIR_CHECK_NAMES,
    ) -> render.LintReportView:
        """Compare ``path`` as the tree holds it with the whole new text in ``after``."""
        selection = _select(checks)
        refusal = proposed_check_refusal(checks)
        if refusal:
            raise OperationFailed(refusal)
        findings = _proposed_findings(ctx, path, after, selection.pairs)
        return render.LintReportView(candidates=(render.LintCandidate(id=path, findings=findings),))

    def _judge(self, ctx: RepoContext, selection: "_Selection") -> Judge:
        if not selection.patch:
            return lambda candidate: _pair_only_findings(candidate, ctx, selection.pairs)
        facts = self._facts(ctx)
        source = cache.effective_source(ctx.symbols, self._extractor)
        return lambda candidate: _findings(candidate, ctx, facts, source, selection)

    def _facts(self, ctx: RepoContext) -> patchlint.RepoFacts:
        """Assemble the pre-patch repository every check compares against.

        The texts come from the same bounded reader the scan used, so a file
        the scan skipped for size is a file the near-duplicate check reports
        as unread rather than one it silently treats as empty.
        """
        scan = refs.scan_repo(ctx.root, self._extractor, source=ctx.symbols)
        index = refs.build_ref_index(scan)
        return patchlint.RepoFacts(
            files=scan.by_path(),
            texts=_texts(ctx.root, scan),
            dependencies=read_declared_dependencies(ctx.root),
            resolver=resolve.build_resolver(scan, index),
        )


@dataclass(frozen=True)
class _Selection:
    patch: frozenset[str]
    pairs: tuple[PairRunner, ...]


def _select(names: Collection[str]) -> _Selection:
    refusal = unknown_check_refusal(names)
    if refusal:
        raise OperationFailed(refusal)
    chosen = [check for check in CHECKS if check.name in names]
    return _Selection(
        patch=frozenset(check.name for check in chosen if check.needs is CheckInput.PATCH),
        pairs=tuple(check.run_pairs for check in chosen if check.run_pairs is not None),
    )


def read_declared_dependencies(root: Path) -> patchlint.DeclaredDependencies:
    """Read ``root``'s dependency manifests.

    ``pyproject.toml`` and every ``requirements*.txt`` at the repository root,
    read through the same bounded reader every other file access in this
    package uses. A manifest that cannot be parsed produces a warning and
    contributes nothing, rather than being treated as an empty one -- an empty
    declared set would make every third-party import in the patch look
    hallucinated.

    Here rather than beside the checks that consume it, because opening a file
    at a repository root is this module's job and :mod:`patchlint` states that
    it opens nothing. The parsing stays there: it takes text and is pure.
    """
    packages: set[str] = set()
    sources: list[str] = []
    warnings: list[str] = []
    requires_python = ""

    resolved_root = root.resolve()

    pyproject = root / patchlint.PYPROJECT_NAME
    if pyproject.is_file():
        read = _read_manifest(pyproject, resolved_root)
        if read.text is None:
            warnings.append(f"{patchlint.PYPROJECT_NAME} not read: {read.skipped}")
        else:
            parse = patchlint.parse_pyproject_dependencies(read.text)
            packages.update(parse.packages)
            warnings.extend(parse.warnings)
            requires_python = parse.requires_python
            if parse.parsed:
                sources.append(patchlint.PYPROJECT_NAME)

    for requirements in sorted(root.glob(patchlint.REQUIREMENTS_GLOB)):
        if not requirements.is_file():
            continue
        read = _read_manifest(requirements, resolved_root)
        if read.text is None:
            warnings.append(f"{requirements.name} not read: {read.skipped}")
            continue
        packages.update(patchlint.parse_requirements(read.text))
        sources.append(requirements.name)

    return patchlint.DeclaredDependencies(
        packages=frozenset(packages),
        sources=tuple(sources),
        warnings=tuple(warnings),
        requires_python=requires_python,
    )


def _read_manifest(path: Path, resolved_root: Path) -> BoundedRead:
    """Read one manifest at the repository root, refusing one that leaves it.

    The containment step ``read_bounded`` names as its precondition and these
    two callers did not perform. ``read_bounded`` follows symlinks on purpose
    -- one symlink policy per module, and the policy is the caller's -- so
    without this a repository whose ``pyproject.toml`` is a symlink to a file
    outside it was read, and whatever package names parsed out of that file
    were reported as this repository's own declared dependencies. A repository
    is a thing people clone from strangers.

    ``file_stays_inside`` rather than ``contained_path`` because the answer
    here is a reason, not an exception: this function's whole contract is that
    a manifest it cannot use contributes a warning and nothing else. Silence
    would be worse than either -- an empty declared set makes every
    third-party import in a patch look hallucinated.
    """
    if not file_stays_inside(path, resolved_root):
        return BoundedRead(path=path, text=None, skipped=MANIFEST_OUTSIDE_ROOT)
    return read_bounded(path)


def load_candidates(target: Path) -> tuple[LintCandidateInput, ...]:
    """Read one patch file, or every file in a directory of them.

    The same two shapes ``patch parse`` accepts, and the same id rule
    ``validate`` uses: one file is one candidate and its stem is its id, so a
    lint report and a verdicts document name the same candidate the same way.

    A directory holds candidate patches and nothing else. There is no
    extension filter, so a stray README or editor swap file becomes a
    candidate and is reported with the parse errors it produces.
    """
    resolved = target.expanduser().resolve()
    if resolved.is_dir():
        files = sorted(entry for entry in resolved.iterdir() if entry.is_file())
        if not files:
            message = f"no patch files in {resolved}: one file per candidate patch"
            raise OperationFailed(message)
        return tuple(_candidate(path) for path in files)

    if not resolved.is_file():
        message = f"{resolved} is neither a patch file nor a directory of them"
        raise InputUnreadable(message)
    return (_candidate(resolved),)


def _candidate(path: Path) -> LintCandidateInput:
    """Read one candidate patch file."""
    return LintCandidateInput(id=path.stem, parsed=load_edits(_text(path, "candidate")))


def load_diff(target: Path) -> LintCandidateInput:
    """Read one unified diff -- a branch's or a pull request's -- as a candidate.

    The review case the SEARCH/REPLACE input cannot serve: the change already
    exists and nobody should have to hand-convert it to lint it. One diff is one
    candidate, and its stem is its id, the same rule
    :func:`load_candidates` uses.

    **What the checks then assume, and it is not what the flow suggests.** They
    compare the diff against the repository as it stands, so the repository has
    to be a checkout of the diff's *base*. A branch with the diff already
    applied is the case that fails quietly rather than loudly --
    :func:`agentless_mcp.core.unidiff.orientation` is what turns it into a
    stated coverage gap, and it runs in :meth:`LintService.lint` because only
    there is the tree's own text available to compare against.
    """
    parsed = unidiff.parse_unified_diff(_text(target, "diff"))
    return LintCandidateInput(id=target.stem, parsed=parsed.result, notes=parsed.notes)


def _text(path: Path, what: str) -> str:
    """Read one input file through the bounded reader, or refuse it."""
    read = read_bounded(path)
    if read.text is None:
        message = f"cannot read {what} {path.name}: {read.skipped}"
        raise InputUnreadable(message)
    return read.text


def _findings(
    candidate: LintCandidateInput,
    ctx: RepoContext,
    facts: patchlint.RepoFacts,
    source: patchlint.FragmentSource,
    selection: _Selection,
) -> tuple[render.LintFinding, ...]:
    """Parse one candidate and run the selected checks over the edits it yielded.

    A block that did not parse is reported as a coverage gap rather than
    skipped: a patch half of which was unreadable, linted silently, is a clean
    report about a patch nobody read.

    A candidate with *any* malformed block is not checked at all. The write
    side refuses such a patch whole -- ``validate`` fails the candidate,
    ``patch apply`` refuses the run -- and findings drawn from the blocks that
    happened to parse would describe a patch that can never be applied. Lint
    still exits 0 and still reports every other candidate: the coverage gap is
    the finding.

    A candidate whose pre-image is not in this tree is suppressed the same way
    and for the same reason. The checks compare a patch against the repository
    as it stands, so a diff already applied to it would be compared against
    itself: every top-level symbol it adds is already in the file, and
    ``shadowing`` would report each one as a name defined twice. That is a
    wrong answer rather than a missing one, which is why it costs the whole
    candidate rather than a line.

    A candidate naming a path outside the repository costs that candidate and
    no other. The refusal used to leave ``lint`` -- so a directory of ten
    model-generated candidates in which one names ``../../etc/passwd``
    produced no report at all, rather than nine reports and one refusal --
    which is the case a model-generated candidate set is most likely to hold.
    """
    admitted = _oriented(_checkable(candidate, ctx.root), facts.texts)
    if not admitted.edits:
        return admitted.rows

    report = patchlint.lint_patch(admitted.edits, facts, source)
    unread = tuple(_unread_row(warning) for warning in report.warnings)
    kept = tuple(
        _row(finding)
        for finding in report.findings
        if finding.check in selection.patch or finding.check == patchlint.CHECK_COVERAGE
    )
    return admitted.rows + unread + kept + _tree_pair_rows(admitted.edits, ctx, selection.pairs)


@dataclass(frozen=True)
class _Admission:
    rows: tuple[render.LintFinding, ...]
    edits: tuple[Edit, ...] = ()


def _checkable(candidate: LintCandidateInput, root: Path) -> _Admission:
    parsed = candidate.parsed
    notes = tuple(_note_row(note) for note in candidate.notes)
    gaps = tuple(
        render.LintFinding(
            check=patchlint.CHECK_COVERAGE,
            severity=patchlint.Severity.NOT_CHECKED.value,
            message=f"not checked: block {error.index} did not parse ({error.reason})",
            path=error.path or "",
            line=0,
            location=error.path or "(repository)",
            evidence=f"block {error.index}",
        )
        for error in parsed.errors
    )
    if gaps:
        return _Admission(notes + gaps)

    if not parsed.edits:
        # A candidate that parsed cleanly to zero edits -- an empty
        # ``edits.json``, or a diff whose every section was a note -- checks
        # nothing, and "no findings" about it would be a clean report about a
        # patch that does nothing. Same phrase ``validate`` fails it with.
        empty = render.LintFinding(
            check=patchlint.CHECK_COVERAGE,
            severity=patchlint.Severity.NOT_CHECKED.value,
            message="not checked: the candidate contains no edits",
            path="",
            line=0,
            location="(repository)",
            evidence="0 edits",
        )
        return _Admission((*notes, empty))

    try:
        edits = _canonical(root, parsed.edits)
    except SecurityRefusal as refusal:
        return _Admission((*notes, _refused_row(refusal)))
    return _Admission(notes, edits)


def _oriented(admission: _Admission, texts: Mapping[str, str]) -> _Admission:
    misoriented = unidiff.orientation(admission.edits, texts)
    if not misoriented:
        return admission
    return _Admission(admission.rows + tuple(_misoriented_row(problem) for problem in misoriented))


def _pair_only_findings(
    candidate: LintCandidateInput, ctx: RepoContext, runners: Sequence[PairRunner]
) -> tuple[render.LintFinding, ...]:
    checkable = _checkable(candidate, ctx.root)
    reads = _working_reads(ctx.root, _scanned_paths(checkable.edits))
    admitted = _oriented(checkable, _lenient_texts(reads))
    if not admitted.edits:
        return admitted.rows
    return admitted.rows + _pair_rows(admitted.edits, reads, runners, ctx.config)


def _tree_pair_rows(
    edits: Sequence[Edit], ctx: RepoContext, runners: Sequence[PairRunner]
) -> tuple[render.LintFinding, ...]:
    if not runners:
        return ()
    reads = _working_reads(ctx.root, _pair_paths(edits))
    return _pair_rows(edits, reads, runners, ctx.config)


def _proposed_findings(
    ctx: RepoContext, path: str, after: Path, runners: Sequence[PairRunner]
) -> tuple[render.LintFinding, ...]:
    try:
        target = contained_path(ctx.root, path)
    except SecurityRefusal as refusal:
        return (_refused_row(refusal),)
    relative = target.relative_to(ctx.root).as_posix()
    if erosion.pair_language(relative) is None:
        return ()

    old = _old_text(relative, _working_read(target))
    if isinstance(old, render.LintFinding):
        return (old,)
    new = decoded_strict(read_bounded_bytes(after))
    if new.text is None:
        return (_side_row(MESSAGES.pair_new_unread, relative, new),)
    return _run_pairs([erosion.TextPair(path=relative, old=old, new=new.text)], runners, ctx.config)


def _edited_paths(edits: Sequence[Edit]) -> list[str]:
    return list(dict.fromkeys(edit.path for edit in edits))


def _scanned_paths(edits: Sequence[Edit]) -> list[str]:
    # The files `refs.scan_repo` would read into `facts.texts`, so orientation
    # refuses a misapplied candidate whole here as it does beside the patch checks.
    return [
        path
        for path in _edited_paths(edits)
        if path != CONFIG_FILENAME and Path(path).suffix in TreeSitterExtractor.SUPPORTED_EXTENSIONS
    ]


def _pair_paths(edits: Sequence[Edit]) -> list[str]:
    return [path for path in _edited_paths(edits) if erosion.pair_language(path) is not None]


def _working_reads(root: Path, paths: Sequence[str]) -> dict[str, BoundedBytes | None]:
    return {path: _working_read(root / path) for path in paths}


def _working_read(path: Path) -> BoundedBytes | None:
    # None is a file the change creates, which is not the same as one that
    # could not be read.
    return read_bounded_bytes(path) if path.exists() else None


def _lenient_texts(reads: Mapping[str, BoundedBytes | None]) -> dict[str, str]:
    texts: dict[str, str] = {}
    for path, read in reads.items():
        text = None if read is None else decoded(read).text
        if text is not None:
            texts[path] = text
    return texts


def _pair_rows(
    edits: Sequence[Edit],
    reads: Mapping[str, BoundedBytes | None],
    runners: Sequence[PairRunner],
    config: ProjectConfig,
) -> tuple[render.LintFinding, ...]:
    rows: list[render.LintFinding] = []
    pairs: list[erosion.TextPair] = []
    for path, path_edits in _by_path(edits).items():
        if erosion.pair_language(path) is None:
            continue
        outcome = _candidate_pair(path, reads[path], path_edits)
        if isinstance(outcome, erosion.TextPair):
            pairs.append(outcome)
        else:
            rows.append(outcome)
    return (*rows, *_run_pairs(pairs, runners, config))


def _by_path(edits: Sequence[Edit]) -> dict[str, list[Edit]]:
    grouped: dict[str, list[Edit]] = {}
    for edit in edits:
        grouped.setdefault(edit.path, []).append(edit)
    return grouped


def _candidate_pair(
    path: str, read: BoundedBytes | None, edits: Sequence[Edit]
) -> erosion.TextPair | render.LintFinding:
    old = _old_text(path, read)
    if isinstance(old, render.LintFinding):
        return old
    if old is None and not edits[0].search:
        contents, pending = {path: edits[0].replace}, edits[1:]
    else:
        contents, pending = ({} if old is None else {path: old}), edits
    applied = apply_edits(pending, contents)
    if not applied.ok:
        return _unapplied_row(path, applied.failures)
    return erosion.TextPair(path=path, old=old, new=applied.new_contents.get(path, contents[path]))


def _old_text(path: str, read: BoundedBytes | None) -> str | render.LintFinding | None:
    if read is None:
        return None
    old = decoded_strict(read)
    if old.text is None:
        return _side_row(MESSAGES.pair_old_unread, path, old)
    return old.text


def _run_pairs(
    pairs: Sequence[erosion.TextPair], runners: Sequence[PairRunner], config: ProjectConfig
) -> tuple[render.LintFinding, ...]:
    return tuple(_pair_row(found) for run in runners for found in run(pairs, config))


def _pair_row(found: erosion.PairFinding) -> render.LintFinding:
    row = _row(found.finding)
    return row if found.detail is None else replace(row, detail=found.detail.as_dict())


def _side_row(template: str, path: str, read: BoundedRead) -> render.LintFinding:
    if read.skipped is None:
        # `BoundedRead` sets exactly one of `text` and `skipped`.
        message = f"bounded read of {read.path} set neither text nor a skip reason"
        raise RuntimeError(message)
    return render.LintFinding(
        check=patchlint.CHECK_COVERAGE,
        severity=patchlint.Severity.NOT_CHECKED.value,
        message=template.format(path=path, reason=read.skipped),
        path=path,
        line=0,
        location=path,
        evidence=read.skipped,
    )


def _unapplied_row(path: str, failures: Sequence[EditOutcome]) -> render.LintFinding:
    return render.LintFinding(
        check=patchlint.CHECK_COVERAGE,
        severity=patchlint.Severity.NOT_CHECKED.value,
        message=MESSAGES.pair_edits_unapplied.format(failed=len(failures), path=path),
        path=path,
        line=0,
        location=path,
        evidence=failures[0].reason,
    )


def _note_row(note: unidiff.DiffNote) -> render.LintFinding:
    """Report one part of the input that parsed but has nothing to check."""
    where = note.path or "(repository)"
    return render.LintFinding(
        check=patchlint.CHECK_COVERAGE,
        severity=patchlint.Severity.NOT_CHECKED.value,
        message=f"not checked: {where}: {note.reason}",
        path=note.path,
        line=0,
        location=where,
        evidence=note.reason,
    )


def _refused_row(refusal: SecurityRefusal) -> render.LintFinding:
    """Report one candidate whose paths left the repository, as a coverage gap.

    The same treatment :func:`_misoriented_row` gives the other "this
    candidate cannot be checked against this tree" case, and for the same
    reason: this service decides nothing, so a candidate it cannot check is a
    row rather than the end of the run. The refusal's own message already
    names what was refused without echoing the raw argument.
    """
    return render.LintFinding(
        check=patchlint.CHECK_COVERAGE,
        severity=patchlint.Severity.NOT_CHECKED.value,
        message=f"not checked: {refusal}",
        path="",
        line=0,
        location="(repository)",
        evidence="path refused",
    )


def _misoriented_row(problem: BlockError) -> render.LintFinding:
    """Report one edit whose pre-image is not in the tree being linted.

    Not phrased as a parse failure, because nothing failed to parse: the diff
    was read exactly right and the repository is the wrong one. The reason it
    carries names the remedy as well as the diagnosis, so a reader is not sent
    back to the guide to find out what to do about it.
    """
    return render.LintFinding(
        check=patchlint.CHECK_COVERAGE,
        severity=patchlint.Severity.NOT_CHECKED.value,
        message=f"not checked: {problem.reason}",
        path=problem.path or "",
        line=0,
        location=problem.path or "(repository)",
        evidence=f"block {problem.index}",
    )


def _unread_row(warning: str) -> render.LintFinding:
    """Report what could not be read about the *repository* as a coverage gap.

    :attr:`patchlint.LintReport.warnings` is not about the patch: a dependency
    manifest that did not parse is the case that matters, because it is also
    the case that silences a whole check. A report that renders findings and
    drops these is a clean report about a repository nobody could read, which
    is the same lie as a clean report about a patch nobody read -- so it
    travels as the coverage gap it is, in the same list, until the view model
    grows a warnings field of its own.
    """
    return render.LintFinding(
        check=patchlint.CHECK_COVERAGE,
        severity=patchlint.Severity.NOT_CHECKED.value,
        message=f"not checked: {warning}",
        path="",
        line=0,
        location="(repository)",
        evidence=warning,
    )


def _canonical(root: Path, edits: Sequence[Edit]) -> tuple[Edit, ...]:
    """Refuse every path outside the root and rewrite the rest as relative.

    The same rule :class:`agentless_mcp.application.patch_service.PatchService`
    applies, and for the same two reasons: a block naming ``../../etc/passwd``
    is refused before anything reads it, and ``app.py`` and ``./app.py`` are
    one file rather than two -- which here decides whether an edit's path finds
    the file's row in the scan at all.
    """
    return tuple(
        replace(edit, path=contained_path(root, edit.path).relative_to(root).as_posix())
        for edit in edits
    )


def _row(finding: patchlint.Finding) -> render.LintFinding:
    """Map one core finding onto the view model the renderers read."""
    return render.LintFinding(
        check=finding.check,
        severity=finding.severity.value,
        message=finding.message,
        path=finding.path,
        line=finding.line,
        location=finding.location,
        evidence=finding.evidence,
    )


def _texts(root: Path, scan: refs.RepoScan) -> dict[str, str]:
    """Read the source of every file the scan parsed."""
    texts: dict[str, str] = {}
    for facts in scan.files:
        read = read_bounded(root / facts.path)
        if read.text is not None:
            texts[facts.path] = read.text
    return texts
