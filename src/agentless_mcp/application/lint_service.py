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

Nothing here decides anything. The report has no verdict, ``lint`` exits 0
whatever it finds, and a candidate whose patch does not parse is reported with
its parse errors rather than skipped -- but it is not *checked*, because the
write side refuses a partly-parsed patch whole and findings about the blocks
that did parse would describe a patch that can never be applied.
"""

from collections.abc import Sequence
from dataclasses import dataclass, replace
from pathlib import Path

from agentless_mcp.application import render
from agentless_mcp.application.patch_service import load_edits
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core import cache, patchlint, refs, resolve, unidiff
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.patches import BlockError, Edit, ParseResult
from agentless_mcp.util.errors import AgentlessError
from agentless_mcp.util.fslimits import contained_path, read_bounded


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


class LintService:
    """Runs the deterministic patch checks for one repository."""

    def __init__(self, extractor: TreeSitterExtractor) -> None:
        self._extractor = extractor

    def lint(
        self,
        ctx: RepoContext,
        candidates: Sequence[LintCandidateInput],
    ) -> render.LintReportView:
        """Lint every candidate against ``ctx``, sharing one scan between them."""
        facts = self._facts(ctx)
        source = cache.effective_source(ctx.symbols, self._extractor)
        return render.LintReportView(
            candidates=tuple(
                render.LintCandidate(
                    id=candidate.id,
                    findings=_findings(candidate, ctx.root, facts, source),
                )
                for candidate in candidates
            )
        )

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
            dependencies=patchlint.read_declared_dependencies(ctx.root),
            resolver=resolve.build_resolver(scan, index),
        )


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
            raise AgentlessError(message)
        return tuple(_candidate(path) for path in files)

    if not resolved.is_file():
        message = f"{resolved} is neither a patch file nor a directory of them"
        raise AgentlessError(message)
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
        raise AgentlessError(message)
    return read.text


def _findings(
    candidate: LintCandidateInput,
    root: Path,
    facts: patchlint.RepoFacts,
    source: patchlint.FragmentSource,
) -> tuple[render.LintFinding, ...]:
    """Parse one candidate and run every check over the edits it yielded.

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
    """
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
        return notes + gaps

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
        return (*notes, empty)

    edits = _canonical(root, parsed.edits)
    misoriented = unidiff.orientation(edits, facts.texts)
    if misoriented:
        return notes + tuple(_misoriented_row(problem) for problem in misoriented)

    report = patchlint.lint_patch(edits, facts, source)
    unread = tuple(_unread_row(warning) for warning in report.warnings)
    return notes + unread + tuple(_row(finding) for finding in report.findings)


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
