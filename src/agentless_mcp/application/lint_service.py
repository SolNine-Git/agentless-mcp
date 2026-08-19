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
its parse errors rather than skipped.
"""

from collections.abc import Sequence
from dataclasses import dataclass, replace
from pathlib import Path

from agentless_mcp.application import render
from agentless_mcp.application.patch_service import load_edits
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core import cache, patchlint, refs, resolve
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.patches import Edit
from agentless_mcp.util.errors import AtlasError
from agentless_mcp.util.fslimits import contained_path, read_bounded


@dataclass(frozen=True)
class LintCandidateInput:
    """One patch to lint: an id for the report, and the text it was read from."""

    id: str
    text: str


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
    """Read one patch file, or every patch file in a directory.

    The same two shapes ``patch parse`` accepts, and the same id rule
    ``validate`` uses: one file is one candidate and its stem is its id, so a
    lint report and a verdicts document name the same candidate the same way.
    """
    resolved = target.expanduser().resolve()
    if resolved.is_dir():
        files = sorted(entry for entry in resolved.iterdir() if entry.is_file())
        if not files:
            message = f"no patch files in {resolved}: one file per candidate patch"
            raise AtlasError(message)
        return tuple(_candidate(path) for path in files)

    if not resolved.is_file():
        message = f"{resolved} is neither a patch file nor a directory of them"
        raise AtlasError(message)
    return (_candidate(resolved),)


def _candidate(path: Path) -> LintCandidateInput:
    """Read one candidate patch file."""
    read = read_bounded(path)
    if read.text is None:
        message = f"cannot read candidate {path.name}: {read.skipped}"
        raise AtlasError(message)
    return LintCandidateInput(id=path.stem, text=read.text)


def _findings(
    candidate: LintCandidateInput,
    root: Path,
    facts: patchlint.RepoFacts,
    source: patchlint.FragmentSource,
) -> tuple[render.LintFinding, ...]:
    """Parse one candidate and run every check over the edits it yielded.

    A block that did not parse is reported as a coverage gap in the same list
    as the findings: a patch half of which was unreadable, linted silently, is
    a clean report about a patch nobody read.
    """
    parsed = load_edits(candidate.text)
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
    report = patchlint.lint_patch(_canonical(root, parsed.edits), facts, source)
    return gaps + tuple(_row(finding) for finding in report.findings)


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
