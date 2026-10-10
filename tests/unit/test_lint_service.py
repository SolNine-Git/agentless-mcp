"""The lint service's two coverage rules: it decides nothing and it survives.

``lint`` has no verdict and exits 0 whatever it finds, so every per-candidate
failure has to become a row in the report rather than the end of the run. The
two cases here are the ones that used to break that: a candidate naming a path
outside the repository, which took the whole run down with it, and a repository
file the reader skipped, which the report has to admit it never compared.
"""

import json
import subprocess

import pytest

from agentless_mcp.application import lint_service
from agentless_mcp.application.lint_service import LintCandidateInput, LintService
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import erosion, gitinfo, patchlint
from agentless_mcp.core.gitinfo import GitOutcome
from agentless_mcp.core.patches import parse_blocks
from agentless_mcp.util import fslimits
from agentless_mcp.util.errors import OperationFailed, RevisionUnresolved

APP = """\
def add(left, right):
    return left + right
"""

UTIL = 'VERSION = "1"\n'

GOOD = """\
### app.py
<<<<<<< SEARCH
    return left + right
=======
    return round(left + right, 2)
>>>>>>> REPLACE
"""

ESCAPING = """\
### ../../etc/passwd
<<<<<<< SEARCH
root
=======
pwned
>>>>>>> REPLACE
"""


@pytest.fixture
def repo(make_git_repo):
    """A committed two-file python repository."""
    return make_git_repo({"app.py": APP, "util.py": UTIL})


@pytest.fixture
def service(extractor):
    """The lint service, wired as bootstrap wires it."""
    return LintService(extractor)


def candidate(name, text):
    """Build one lint input from raw SEARCH/REPLACE text."""
    return LintCandidateInput(id=name, parsed=parse_blocks(text))


def by_id(view):
    """Index one report's candidates by their id."""
    return {entry.id: entry for entry in view.candidates}


class TestOneRefusedCandidateCostsOnlyItself:
    """A path escape is a row, not the end of the run.

    ``_canonical`` raised out of the generator that builds the report, so a
    directory of ten model-generated candidates in which one names
    ``../../etc/passwd`` produced no report at all rather than nine reports
    and one refusal -- and that is the case a model-generated candidate set is
    most likely to hold. Every other per-candidate failure already degrades
    into a coverage row.
    """

    def test_the_run_survives_and_reports_every_other_candidate(self, service, repo):
        view = service.lint(
            resolve_repo(repo, None),
            [candidate("01-good", GOOD), candidate("02-escape", ESCAPING)],
        )

        assert set(by_id(view)) == {"01-good", "02-escape"}

    def test_the_refused_candidate_carries_the_reason_as_a_coverage_gap(self, service, repo):
        view = service.lint(resolve_repo(repo, None), [candidate("02-escape", ESCAPING)])

        (finding,) = by_id(view)["02-escape"].findings
        assert finding.check == patchlint.CHECK_COVERAGE
        assert finding.severity == patchlint.Severity.NOT_CHECKED.value
        assert "outside the root" in finding.message
        assert finding.evidence == "path refused"

    def test_the_row_never_echoes_the_raw_path(self, service, repo):
        view = service.lint(resolve_repo(repo, None), [candidate("02-escape", ESCAPING)])

        (finding,) = by_id(view)["02-escape"].findings
        assert "../../etc/passwd" not in finding.message


class TestAFileTheReaderSkippedIsReportedUnread:
    """A file the scan parsed and the reader skipped is not an empty file.

    ``_facts`` says the texts come from the same bounded reader the scan used
    "so a file the scan skipped for size is a file the near-duplicate check
    reports as unread rather than one it silently treats as empty". The skip
    arm is what makes that true, and it had no test at this layer.
    """

    def test_the_near_duplicate_check_says_what_it_could_not_compare(
        self, service, repo, monkeypatch
    ):
        real = fslimits.read_bounded

        def skip_util(path, *arguments, **keywords):
            if path.name == "util.py":
                return fslimits.BoundedRead(path=path, text=None, skipped="skipped: too big")
            return real(path, *arguments, **keywords)

        monkeypatch.setattr(lint_service, "read_bounded", skip_util)

        view = service.lint(resolve_repo(repo, None), [candidate("01-good", GOOD)])

        messages = [finding.message for finding in by_id(view)["01-good"].findings]
        assert any("had no text supplied" in message for message in messages)
        assert any(
            "first unread: util.py" in finding.evidence
            for finding in by_id(view)["01-good"].findings
        )

    def test_with_every_file_read_nothing_is_reported_unread(self, service, repo):
        view = service.lint(resolve_repo(repo, None), [candidate("01-good", GOOD)])

        messages = [finding.message for finding in by_id(view)["01-good"].findings]
        assert not any("had no text supplied" in message for message in messages)


class TestAManifestSymlinkedOutOfTheRepositoryIsRefused:
    """A repository is a thing people clone from strangers.

    ``read_bounded`` follows symlinks deliberately and states containment as
    its caller's job. These two reads did not do it, so a repository whose
    ``pyproject.toml`` was a symlink to any file the user could read had that
    file read and its parsed package names reported as the repository's own
    declared dependencies. Reproduced during the audit; the base commit
    refused it only by accident, through an ``O_NOFOLLOW`` that also refused
    every legitimate in-repository symlink.
    """

    def test_a_symlinked_pyproject_contributes_nothing_and_says_so(self, tmp_path):
        outside = tmp_path / "outside.toml"
        outside.write_text(
            '[project]\nname = "x"\ndependencies = ["exfiltrated"]\n', encoding="utf-8"
        )
        root = tmp_path / "repo"
        root.mkdir()
        (root / patchlint.PYPROJECT_NAME).symlink_to(outside)

        declared = lint_service.read_declared_dependencies(root)

        assert "exfiltrated" not in declared.packages
        assert declared.sources == ()
        assert declared.warnings == (
            f"{patchlint.PYPROJECT_NAME} not read: {lint_service.MANIFEST_OUTSIDE_ROOT}",
        )

    def test_a_symlinked_requirements_file_contributes_nothing_and_says_so(self, tmp_path):
        outside = tmp_path / "outside.txt"
        outside.write_text("exfiltrated\n", encoding="utf-8")
        root = tmp_path / "repo"
        root.mkdir()
        (root / "requirements.txt").symlink_to(outside)

        declared = lint_service.read_declared_dependencies(root)

        assert "exfiltrated" not in declared.packages
        assert declared.sources == ()
        assert declared.warnings == (
            f"requirements.txt not read: {lint_service.MANIFEST_OUTSIDE_ROOT}",
        )

    def test_a_manifest_symlinked_within_the_repository_is_still_read(self, tmp_path):
        # The containment rule, not a blanket symlink refusal: this is exactly
        # the case `O_NOFOLLOW` used to get wrong in the other direction.
        root = tmp_path / "repo"
        root.mkdir()
        (root / "real.toml").write_text(
            '[project]\nname = "x"\ndependencies = ["declared"]\n', encoding="utf-8"
        )
        (root / patchlint.PYPROJECT_NAME).symlink_to(root / "real.toml")

        declared = lint_service.read_declared_dependencies(root)

        assert "declared" in declared.packages
        assert declared.sources == (patchlint.PYPROJECT_NAME,)
        assert declared.warnings == ()


def function(name, cc, tag=""):
    """Python source for one function of complexity ``cc``."""
    lines = [f"def {name}(value):"]
    for index in range(cc - 1):
        lines += [f"    if value == {index}:", f"        return {index}"]
    lines.append(f'    return "{tag}"')
    return "\n".join(lines) + "\n"


def block(path, search, replace):
    """One SEARCH/REPLACE block as text; both sides end with a newline or are empty."""
    return f"### {path}\n<<<<<<< SEARCH\n{search}=======\n{replace}>>>>>>> REPLACE\n"


ALL = lint_service.CHECK_NAMES
PAIRS_ONLY = (erosion.CHECK_COMPLEXITY_DELTA,)
BOTH_MODES = pytest.mark.parametrize("checks", [ALL, PAIRS_ONLY], ids=["all-checks", "pairs-only"])
CROSSING = block("app.py", function("handle", 10), function("handle", 11))


@pytest.fixture
def erosion_repo(make_git_repo):
    """A committed repository with one function at the threshold and two odd files."""
    return make_git_repo(
        {
            "app.py": function("handle", 10),
            "empty.py": "",
            "twice.py": "x = 1\n\nx = 1\n",
            "config.yaml": "key: value\n",
        },
        name="erosion",
    )


def findings_of(service, root, text, checks):
    """Lint one candidate and return its findings."""
    view = service.lint(resolve_repo(root, None), [candidate("01", text)], checks)
    (entry,) = view.candidates
    return entry.findings


def checks_in(findings):
    """The check name of each finding, in order."""
    return [finding.check for finding in findings]


class TestTheCheckRegistry:
    def test_every_patchlint_check_is_registered_as_a_patch_check(self):
        declared = {
            getattr(patchlint, name)
            for name in dir(patchlint)
            if name.startswith("CHECK_") and name != "CHECK_COVERAGE"
        }
        registered = {
            check.name
            for check in lint_service.CHECKS
            if check.needs is lint_service.CheckInput.PATCH
        }
        assert registered == declared

    def test_the_three_erosion_checks_are_the_pair_checks(self):
        assert lint_service.PAIR_CHECK_NAMES == (
            erosion.CHECK_COMPLEXITY_DELTA,
            erosion.CHECK_CONTRACT_DRIFT,
            erosion.CHECK_CLONE_DELTA,
        )

    def test_the_hook_and_review_default_reads_only_the_edited_files(self):
        assert lint_service.EDITED_FILE_CHECK_NAMES == (
            erosion.CHECK_COMPLEXITY_DELTA,
            erosion.CHECK_CONTRACT_DRIFT,
        )

    def test_clone_delta_alone_reads_the_clone_index(self):
        needs = {check.name: check.needs for check in lint_service.CHECKS}

        assert needs[erosion.CHECK_CLONE_DELTA] is lint_service.CheckInput.PAIRS_CLONES
        assert lint_service.CheckInput.PAIRS_CLONES.value == "pairs+clones"

    @pytest.mark.parametrize(
        ("needs", "runner"),
        [
            (lint_service.CheckInput.PAIRS, None),
            (lint_service.CheckInput.PAIRS_CLONES, None),
            (lint_service.CheckInput.PATCH, lambda _run: ()),
        ],
        ids=["pair-check-without-runner", "clone-check-without-runner", "patch-check-with-runner"],
    )
    def test_a_check_and_its_runner_must_agree(self, needs, runner):
        with pytest.raises(ValueError, match="a pair check needs a runner"):
            lint_service.LintCheck("x", needs, runner)

    def test_an_unknown_check_name_is_refused(self, service, erosion_repo):
        with pytest.raises(OperationFailed, match="unknown check 'complexity'"):
            findings_of(service, erosion_repo, CROSSING, ("complexity",))

    def test_an_empty_selection_is_refused(self, service, erosion_repo):
        with pytest.raises(OperationFailed, match="no check selected"):
            findings_of(service, erosion_repo, CROSSING, ())


class TestOnlyPairChecksSkipTheScan:
    def test_neither_the_scan_nor_the_patch_checks_run(self, service, erosion_repo, monkeypatch):
        def forbidden(*_arguments, **_keywords):
            message = "a pair-only run must not reach this"
            raise AssertionError(message)

        monkeypatch.setattr(LintService, "_facts", forbidden)
        monkeypatch.setattr(patchlint, "lint_patch", forbidden)

        findings = findings_of(service, erosion_repo, CROSSING, PAIRS_ONLY)

        assert checks_in(findings) == [erosion.CHECK_COMPLEXITY_DELTA]

    def test_a_patch_check_subset_keeps_only_its_rows_and_the_coverage_rows(
        self, service, erosion_repo
    ):
        findings = findings_of(service, erosion_repo, CROSSING, (patchlint.CHECK_CYCLE_DELTA,))

        assert set(checks_in(findings)) <= {patchlint.CHECK_CYCLE_DELTA, patchlint.CHECK_COVERAGE}

    def test_all_checks_put_the_pair_rows_after_the_patch_rows(self, service, erosion_repo):
        findings = findings_of(service, erosion_repo, CROSSING, ALL)

        assert checks_in(findings) == [
            patchlint.CHECK_UNDECLARED_IMPORTS,
            erosion.CHECK_COMPLEXITY_DELTA,
            erosion.CHECK_CONTRACT_DRIFT,
        ]


class TestCandidatePairs:
    @BOTH_MODES
    def test_an_edit_that_crosses_the_threshold_is_reported(self, service, erosion_repo, checks):
        (found,) = [
            finding
            for finding in findings_of(service, erosion_repo, CROSSING, checks)
            if finding.check == erosion.CHECK_COMPLEXITY_DELTA
        ]

        assert found.location == "app.py:1"
        assert found.evidence == "cc 10 -> 11; sloc 20 -> 22; threshold 10"
        assert found.detail == {
            "unit": "handle",
            "match": "name",
            "cc_old": 10,
            "cc_new": 11,
            "sloc_old": 20,
            "sloc_new": 22,
            "threshold": 10,
        }

    @BOTH_MODES
    def test_an_empty_search_on_a_missing_path_creates_the_file(
        self, service, erosion_repo, checks
    ):
        text = block("pkg/new.py", "", function("fresh", 12))

        rows = findings_of(service, erosion_repo, text, checks)

        (found,) = [row for row in rows if row.check == erosion.CHECK_COMPLEXITY_DELTA]
        assert found.location == "pkg/new.py:1"
        assert found.detail["match"] == "new"

    @BOTH_MODES
    def test_an_empty_search_on_an_existing_empty_file_is_misoriented(
        self, service, erosion_repo, checks
    ):
        rows = findings_of(service, erosion_repo, block("empty.py", "", function("x", 12)), checks)

        (row,) = rows
        assert row.check == patchlint.CHECK_COVERAGE
        assert "already exists in --repo" in row.message

    @BOTH_MODES
    def test_an_edit_that_does_not_apply_is_a_coverage_row(self, service, erosion_repo, checks):
        rows = findings_of(service, erosion_repo, block("twice.py", "x = 1\n", "x = 2\n"), checks)

        (row,) = [row for row in rows if row.path == "twice.py"]
        assert row.check == patchlint.CHECK_COVERAGE
        assert row.message == (
            "not checked: 1 of the edits to twice.py did not apply, so no pair check compared "
            "this file."
        )

    @BOTH_MODES
    def test_a_file_over_the_cap_is_a_coverage_row(self, service, erosion_repo, checks):
        filler = ("# " + "x" * 77 + "\n") * 13_000
        (erosion_repo / "huge.py").write_text(filler + "def tail():\n    return 1\n")

        rows = findings_of(
            service, erosion_repo, block("huge.py", "    return 1\n", "    return 2\n"), checks
        )

        (row,) = [row for row in rows if row.check == patchlint.CHECK_COVERAGE]
        assert row.message.startswith("not checked: the text of huge.py before the change was not")
        assert row.evidence.startswith("skipped: ")

    @BOTH_MODES
    def test_a_file_that_is_not_utf8_is_a_coverage_row(self, service, erosion_repo, checks):
        (erosion_repo / "latin.py").write_bytes(b"y = 1\nx = '\xff'\n")

        rows = findings_of(service, erosion_repo, block("latin.py", "y = 1\n", "y = 2\n"), checks)

        (row,) = [row for row in rows if row.path == "latin.py"]
        assert row.evidence == "not UTF-8 text: byte 11 is invalid start byte"

    @BOTH_MODES
    def test_a_path_outside_the_repository_is_refused(self, service, erosion_repo, checks):
        (row,) = findings_of(service, erosion_repo, ESCAPING, checks)

        assert row.evidence == "path refused"

    @BOTH_MODES
    def test_a_misapplied_edit_to_another_scanned_file_suppresses_the_candidate(
        self, service, erosion_repo, checks
    ):
        """Pair-only orientation reads the files the scan reads, not only the pair files."""
        text = CROSSING + "\n" + block("config.yaml", "missing: line\n", "other: line\n")

        rows = findings_of(service, erosion_repo, text, checks)

        (row,) = rows
        assert row.path == "config.yaml"
        assert "is not against --repo" in row.message

    def test_the_configured_threshold_applies(self, service, make_git_repo):
        root = make_git_repo(
            {"app.py": function("handle", 3), ".agentless-mcp.json": '{"complexity_threshold": 3}'},
            name="configured",
        )

        (found,) = findings_of(
            service, root, block("app.py", function("handle", 3), function("handle", 4)), PAIRS_ONLY
        )

        assert found.detail["threshold"] == 3


class TestTheHookForm:
    """`lint_proposed`: the working-tree text at a path against a whole new text."""

    def proposed(self, service, root, path, after, checks=PAIRS_ONLY):
        view = service.lint_proposed(resolve_repo(root, None), path, after, checks)
        (entry,) = view.candidates
        return entry

    def test_a_crossing_change_is_reported_under_the_path_as_given(
        self, service, erosion_repo, tmp_path
    ):
        after = tmp_path / "after.py"
        after.write_text(function("handle", 11))

        entry = self.proposed(service, erosion_repo, "./app.py", after)

        assert entry.id == "./app.py"
        (found,) = entry.findings
        assert found.location == "app.py:1"
        assert found.detail["match"] == "name"

    @pytest.mark.parametrize("path", ["new.py", "pkg/sub/new.py"])
    def test_a_path_that_does_not_exist_is_a_new_file(self, service, erosion_repo, tmp_path, path):
        after = tmp_path / "after.py"
        after.write_text(function("fresh", 11))

        (found,) = self.proposed(service, erosion_repo, path, after).findings

        assert found.path == path
        assert found.detail["match"] == "new"

    def test_an_empty_proposed_text_reports_nothing(self, service, erosion_repo, tmp_path):
        after = tmp_path / "after.py"
        after.write_text("")

        assert self.proposed(service, erosion_repo, "app.py", after).findings == ()

    def test_both_sides_reach_the_check_with_their_line_endings(
        self, service, erosion_repo, tmp_path, monkeypatch
    ):
        seen = []
        real = erosion.complexity_delta

        def spy(pairs, threshold):
            seen.extend(pairs)
            return real(pairs, threshold)

        monkeypatch.setattr(erosion, "complexity_delta", spy)
        (erosion_repo / "app.py").write_bytes(b"x = 1\r\ny = 2\r\n")
        after = tmp_path / "after.py"
        after.write_bytes(b"x = 1\r\ny = 3\r\n")

        self.proposed(service, erosion_repo, "app.py", after)

        assert seen == [erosion.TextPair("app.py", "x = 1\r\ny = 2\r\n", "x = 1\r\ny = 3\r\n")]

    @pytest.mark.parametrize("side", ["old", "new"])
    def test_a_side_over_the_cap_is_a_coverage_row(self, service, erosion_repo, tmp_path, side):
        oversized = ("# " + "x" * 77 + "\n") * 13_000
        after = tmp_path / "after.py"
        after.write_text(oversized if side == "new" else "x = 1\n")
        if side == "old":
            (erosion_repo / "app.py").write_text(oversized)

        (row,) = self.proposed(service, erosion_repo, "app.py", after).findings

        assert row.check == patchlint.CHECK_COVERAGE
        assert row.evidence.startswith("skipped: ")
        expected = "before the change" if side == "old" else "the proposed text for app.py"
        assert expected in row.message

    def test_a_proposed_text_that_is_not_utf8_is_a_coverage_row(
        self, service, erosion_repo, tmp_path
    ):
        after = tmp_path / "after.py"
        after.write_bytes(b"x = '\xff'\n")

        (row,) = self.proposed(service, erosion_repo, "app.py", after).findings

        assert row.evidence == "not UTF-8 text: byte 5 is invalid start byte"

    def test_a_missing_proposed_file_is_a_coverage_row_not_a_deletion(
        self, service, erosion_repo, tmp_path
    ):
        (row,) = self.proposed(service, erosion_repo, "app.py", tmp_path / "gone.py").findings

        assert row.check == patchlint.CHECK_COVERAGE
        assert row.evidence.startswith("unreadable: ")

    def test_a_path_outside_the_repository_is_refused(self, service, erosion_repo, tmp_path):
        after = tmp_path / "after.py"
        after.write_text("x = 1\n")

        (row,) = self.proposed(service, erosion_repo, "../outside.py", after).findings

        assert row.evidence == "path refused"
        assert "../outside.py" not in row.message

    def test_a_symlink_out_of_the_repository_is_refused(self, service, erosion_repo, tmp_path):
        outside = tmp_path / "outside.py"
        outside.write_text(function("handle", 2))
        (erosion_repo / "link.py").symlink_to(outside)
        after = tmp_path / "after.py"
        after.write_text(function("handle", 12))

        (row,) = self.proposed(service, erosion_repo, "link.py", after).findings

        assert row.evidence == "path refused"

    def test_a_language_without_complexity_rules_gives_no_row(
        self, service, erosion_repo, tmp_path
    ):
        after = tmp_path / "README.md"
        after.write_text("# notes\n")

        assert self.proposed(service, erosion_repo, "README.md", after).findings == ()

    def test_the_hook_form_runs_no_scan(self, service, erosion_repo, tmp_path, monkeypatch):
        def forbidden(*_arguments, **_keywords):
            message = "the hook form must not reach this"
            raise AssertionError(message)

        monkeypatch.setattr(LintService, "_facts", forbidden)
        monkeypatch.setattr(patchlint, "lint_patch", forbidden)
        after = tmp_path / "after.py"
        after.write_text(function("handle", 11))

        assert self.proposed(service, erosion_repo, "app.py", after).findings

    def test_a_patch_check_is_refused(self, service, erosion_repo, tmp_path):
        with pytest.raises(OperationFailed, match="--path runs only the pair checks"):
            self.proposed(service, erosion_repo, "app.py", tmp_path / "x.py", checks=("arity",))


CONTRACT_ONLY = (erosion.CHECK_CONTRACT_DRIFT,)


@pytest.fixture
def boundary_repo(make_git_repo):
    return make_git_repo(
        {
            ".agentless-mcp.json": json.dumps({"boundary_globs": ["adapters/**"]}),
            "adapters/api.py": "def parse(data):\n    return data['items']\n",
            "core.py": "def total(items):\n    return sum(items)\n",
        },
        name="boundary",
    )


class TestContractDrift:
    def test_the_hook_form_reports_a_new_hit_in_a_boundary_module(
        self, service, boundary_repo, tmp_path
    ):
        after = tmp_path / "after.py"
        after.write_text("def parse(data):\n    return data.get('items', [])\n")

        view = service.lint_proposed(
            resolve_repo(boundary_repo, None), "adapters/api.py", after, CONTRACT_ONLY
        )

        (entry,) = view.candidates
        (found,) = entry.findings
        assert (found.check, found.severity) == (erosion.CHECK_CONTRACT_DRIFT, "warning")
        assert found.location == "adapters/api.py:2"
        assert found.detail == {
            "rule": "get-default-empty",
            "unit": "parse",
            "capture": "data.get('items', [])",
        }

    def test_a_boundary_file_without_rules_carries_its_language(
        self, service, boundary_repo, tmp_path
    ):
        after = tmp_path / "run.sh"
        after.write_text("echo hi\n")

        view = service.lint_proposed(
            resolve_repo(boundary_repo, None), "adapters/run.sh", after, CONTRACT_ONLY
        )

        (entry,) = view.candidates
        (gap,) = entry.findings
        assert gap.severity == "not_checked"
        assert gap.as_dict()["detail"] == {"reason": "no_rules_for_language", "language": "bash"}

    def test_an_unreadable_side_carries_no_detail(self, service, boundary_repo, tmp_path):
        after = tmp_path / "after.py"
        after.write_bytes(b"x = '\xff'\n")

        view = service.lint_proposed(
            resolve_repo(boundary_repo, None), "adapters/api.py", after, CONTRACT_ONLY
        )

        (entry,) = view.candidates
        (gap,) = entry.findings
        assert gap.severity == "not_checked"
        assert "detail" not in gap.as_dict()

    def test_an_edit_outside_the_boundary_gives_no_row(self, service, boundary_repo):
        text = block("core.py", "    return sum(items)\n", "    return sum(items or [])\n")
        assert findings_of(service, boundary_repo, text, CONTRACT_ONLY) == ()

    def test_without_globs_a_candidate_gets_one_row_for_all_its_files(self, service, erosion_repo):
        text = block("app.py", function("handle", 10), function("handle", 2)) + block(
            "twice.py", "x = 1\n\nx = 1\n", "x = 1\n\nx = y or []\n"
        )

        (gap,) = findings_of(service, erosion_repo, text, CONTRACT_ONLY)

        assert (gap.check, gap.severity, gap.location) == (
            erosion.CHECK_CONTRACT_DRIFT,
            "not_checked",
            "(repository)",
        )
        assert gap.as_dict()["detail"] == {"reason": "no_boundary_globs"}

    def test_without_globs_a_candidate_with_no_tier1_file_gets_no_row(self, service, erosion_repo):
        text = block("config.yaml", "key: value\n", "key: other\n")
        assert findings_of(service, erosion_repo, text, CONTRACT_ONLY) == ()


class TestObservedInEveryForm:
    """`observed` describes the new text in the candidate, hook, and review forms."""

    def entry(self, service, root, text, checks=PAIRS_ONLY):
        view = service.lint(resolve_repo(root, None), [candidate("01", text)], checks)
        (found,) = view.candidates
        return found

    def test_a_function_moved_between_files_of_a_candidate_is_neither_new_nor_changed(
        self, service, make_git_repo
    ):
        big = function("big", 15)
        root = make_git_repo({"app.py": big + "\n" + function("small", 2)}, name="moving")
        text = block("app.py", big + "\n", "") + block("other.py", "", big)

        entry = self.entry(service, root, text)

        assert entry.findings == ()
        assert entry.observed["units_changed"] == []
        assert entry.observed["units_over_threshold"] == [
            {"path": "other.py", "unit": "big", "line": 1, "cc": 15, "sloc": 30}
        ]

    def test_a_renamed_function_is_not_listed_as_changed(self, service, make_git_repo):
        root = make_git_repo({"app.py": function("handle", 12)}, name="renaming")
        text = block("app.py", "def handle(value):\n", "def renamed(value):\n")

        observed = self.entry(service, root, text).observed

        assert observed["units_changed"] == []
        assert [row["unit"] for row in observed["units_over_threshold"]] == ["renamed"]

    def test_the_hook_form_lists_a_grown_function(self, service, erosion_repo, tmp_path):
        after = tmp_path / "after.py"
        after.write_text(function("handle", 11))

        view = service.lint_proposed(resolve_repo(erosion_repo, None), "app.py", after)

        (entry,) = view.candidates
        assert entry.observed["units_changed"] == [
            {
                "path": "app.py",
                "unit": "handle",
                "line": 1,
                "match": "name",
                "cc_old": 10,
                "cc_new": 11,
                "sloc_old": 20,
                "sloc_new": 22,
            }
        ]
        assert "contract_keys" not in entry.observed

    def test_the_hook_form_observes_contract_keys_in_a_boundary_file(
        self, service, boundary_repo, tmp_path
    ):
        after = tmp_path / "after.py"
        after.write_text("def parse(data):\n    return data.get('items', [])\n")

        view = service.lint_proposed(resolve_repo(boundary_repo, None), "adapters/api.py", after)

        (entry,) = view.candidates
        assert entry.as_dict()["observed"]["contract_keys"] == [
            {
                "path": "adapters/api.py",
                "rule": "get-default-empty",
                "unit": "parse",
                "capture": "data.get('items', [])",
            }
        ]

    def test_a_candidate_that_measured_nothing_has_no_observed_field(self, service, erosion_repo):
        entry = self.entry(service, erosion_repo, block("config.yaml", "key: value\n", "key: x\n"))

        assert entry.observed is None
        assert "observed" not in entry.as_dict()

    def test_the_review_form_observes_the_working_tree(self, service, erosion_repo):
        (erosion_repo / "app.py").write_text(function("handle", 11))

        (entry,) = service.lint_revision(resolve_repo(erosion_repo, None), "HEAD").candidates

        assert [row["unit"] for row in entry.observed["units_changed"]] == ["handle"]
        assert [row["unit"] for row in entry.observed["units_over_threshold"]] == ["handle"]


IDENTITY = ("-c", "user.email=tests@example.invalid", "-c", "user.name=agentless-mcp tests")


def git(root, *arguments):
    """Run one git command in ``root`` with the pinned test identity, failing loudly."""
    return subprocess.run(
        ["git", "-C", str(root), *IDENTITY, *arguments],
        check=True,
        capture_output=True,
        timeout=30,
    ).stdout


def commit(root):
    """Commit the whole tree."""
    git(root, "add", "-A")
    git(root, "commit", "-q", "-m", "change", "-m", "A second commit for a review test.")


def review(service, root, revision="HEAD", checks=PAIRS_ONLY):
    """Review ``root`` against ``revision`` and return its one candidate."""
    (entry,) = service.lint_revision(resolve_repo(root, None), revision, checks).candidates
    return entry


def js_function(name, cc):
    """JavaScript source for one function of complexity ``cc``."""
    branches = "".join(
        f"  if (v === {index}) {{\n    return {index};\n  }}\n" for index in range(cc - 1)
    )
    return f"function {name}(v) {{\n{branches}  return -1;\n}}\n"


def only_row(entry):
    """The one finding of a review, which must be a coverage row."""
    (row,) = entry.findings
    assert (row.check, row.severity) == (patchlint.CHECK_COVERAGE, "not_checked")
    return row


class TestReviewMode:
    """`lint_revision`: every file changed since a commit against its working-tree text."""

    def test_a_modified_file_that_crosses_the_threshold_is_reported(self, service, erosion_repo):
        (erosion_repo / "app.py").write_text(function("handle", 11))

        entry = review(service, erosion_repo)

        assert entry.id == "HEAD"
        (found,) = entry.findings
        assert (found.check, found.location) == (erosion.CHECK_COMPLEXITY_DELTA, "app.py:1")
        assert found.detail["match"] == "name"

    def test_a_renamed_file_is_compared_at_its_new_path(self, service, erosion_repo):
        git(erosion_repo, "mv", "app.py", "moved.py")
        (erosion_repo / "moved.py").write_text(function("handle", 11))

        found = next(row for row in review(service, erosion_repo).findings)

        assert found.location == "moved.py:1"
        assert (found.detail["match"], found.detail["cc_old"]) == ("name", 10)

    def test_a_deleted_file_gives_no_row(self, service, erosion_repo):
        (erosion_repo / "app.py").unlink()

        entry = review(service, erosion_repo)

        assert entry.findings == ()
        assert entry.observed == {"units_over_threshold": [], "units_changed": []}

    def test_a_function_moved_out_of_a_deleted_file_is_not_a_finding(self, service, make_git_repo):
        root = make_git_repo({"app.py": function("big", 15)}, name="moved-out")
        (root / "app.py").unlink()
        (root / "other.py").write_text(function("big", 15))

        entry = review(service, root)

        assert entry.findings == ()
        assert entry.observed["units_changed"] == []

    def test_a_staged_file_and_an_untracked_one_report_their_functions_as_new(
        self, service, erosion_repo
    ):
        (erosion_repo / "staged.py").write_text(function("staged", 11))
        git(erosion_repo, "add", "staged.py")
        (erosion_repo / "loose.py").write_text(function("loose", 11))

        findings = review(service, erosion_repo).findings

        assert [(found.path, found.detail["match"]) for found in findings] == [
            ("loose.py", "new"),
            ("staged.py", "new"),
        ]

    def test_committed_and_uncommitted_changes_both_count(self, service, erosion_repo):
        (erosion_repo / "app.py").write_text(function("handle", 11))
        commit(erosion_repo)
        (erosion_repo / "loose.py").write_text(function("loose", 11))

        findings = review(service, erosion_repo, "HEAD~1").findings

        assert [found.path for found in findings] == ["app.py", "loose.py"]

    def test_a_binary_file_gives_one_row(self, service, make_git_repo):
        root = make_git_repo({"blob.py": "x = 1\n"}, name="binary")
        (root / "blob.py").write_bytes(b"x = 1\0\n")

        row = only_row(review(service, root))

        assert row.location == "blob.py"
        assert row.message == "not checked: blob.py is binary, so no pair check compared this file."
        assert row.evidence == "a NUL byte in the first 8000 bytes"

    def test_a_submodule_gives_one_row(self, service, erosion_repo):
        head = git(erosion_repo, "rev-parse", "HEAD").decode().strip()
        git(erosion_repo, "update-index", "--add", "--cacheinfo", f"160000,{head},vendor/lib")
        (erosion_repo / "vendor" / "lib").mkdir(parents=True)

        row = only_row(review(service, erosion_repo))

        assert row.location == "vendor/lib"
        assert row.evidence == "submodule (mode 160000)"
        assert "is a submodule" in row.message

    def test_an_untracked_nested_repository_gives_one_row(self, service, erosion_repo):
        nested = erosion_repo / "nested"
        nested.mkdir()
        git(nested, "init", "-q")
        (nested / "inner.py").write_text("x = 1\n")
        commit(nested)

        row = only_row(review(service, erosion_repo))

        assert row.location == "nested"
        assert row.evidence == "submodule (mode 160000)"

    def test_a_file_removed_from_the_index_but_kept_on_disk_is_one_comparison(
        self, service, erosion_repo
    ):
        git(erosion_repo, "rm", "-q", "--cached", "app.py")
        (erosion_repo / "app.py").write_text(function("handle", 11))

        (found,) = review(service, erosion_repo).findings

        assert (found.location, found.detail["match"]) == ("app.py:1", "name")

    def test_a_file_outside_the_tier1_set_gives_no_row(self, service, erosion_repo):
        (erosion_repo / "config.yaml").write_text("key: other\n")

        entry = review(service, erosion_repo)

        assert entry.findings == ()
        assert "observed" not in entry.as_dict()

    def test_no_change_gives_one_row(self, service, erosion_repo):
        row = only_row(review(service, erosion_repo))

        assert (
            row.message
            == "not checked: no file changed since HEAD, so there is nothing to compare."
        )
        assert row.location == "(repository)"

    def test_a_symlink_that_replaced_a_file_reads_as_a_deletion(self, service, make_git_repo):
        root = make_git_repo({"app.py": function("big", 15), "keep.py": "x = 1\n"}, name="link")
        (root / "app.py").unlink()
        (root / "app.py").symlink_to("keep.py")
        (root / "alias.py").symlink_to("keep.py")

        entry = review(service, root)

        assert entry.findings == ()
        assert entry.observed == {"units_over_threshold": [], "units_changed": []}

    @pytest.mark.parametrize("revision", ["no-such-branch", "HEAD^{tree}", "--output=x"])
    def test_a_revision_that_names_no_commit_is_refused(self, service, erosion_repo, revision):
        with pytest.raises(RevisionUnresolved, match="does not name a commit"):
            review(service, erosion_repo, revision)

    def test_a_patch_check_is_refused(self, service, erosion_repo):
        with pytest.raises(OperationFailed, match="--since runs only the pair checks"):
            review(service, erosion_repo, checks=("arity",))

    def test_a_contract_hit_moved_between_boundary_files_is_not_new(self, service, boundary_repo):
        (boundary_repo / "adapters" / "api.py").write_text(
            "def parse(data):\n    return data.get('items', [])\n"
        )
        commit(boundary_repo)
        (boundary_repo / "adapters" / "api.py").write_text("VERSION = 2\n")
        (boundary_repo / "adapters" / "parsing.py").write_text(
            "def parse(data):\n    return data.get('items', [])\n"
        )

        entry = review(service, boundary_repo, checks=CONTRACT_ONLY)

        assert entry.findings == ()
        assert entry.observed["contract_keys"] == [
            {
                "path": "adapters/parsing.py",
                "rule": "get-default-empty",
                "unit": "parse",
                "capture": "data.get('items', [])",
            }
        ]

    def test_a_cross_language_rename_is_a_deletion_and_an_addition(self, service, erosion_repo):
        git(erosion_repo, "mv", "app.py", "app.js")
        (erosion_repo / "app.js").write_text(js_function("handle", 11))

        (found,) = review(service, erosion_repo).findings

        assert (found.location, found.detail["match"]) == ("app.js:1", "new")

    def test_a_root_below_the_git_top_level_reads_paths_relative_to_it(
        self, service, make_git_repo
    ):
        root = make_git_repo({"pkg/app.py": function("handle", 10)}, name="nested-root")
        (root / "pkg" / "app.py").write_text(function("handle", 11))
        (root / "pkg" / "loose.py").write_text(function("loose", 11))

        findings = review(service, root / "pkg").findings

        assert [found.location for found in findings] == ["app.py:1", "loose.py:1"]

    def test_the_old_side_keeps_its_line_endings(self, service, erosion_repo, monkeypatch):
        (erosion_repo / "crlf.py").write_bytes(b"x = 1\r\ny = 2\r\n")
        git(erosion_repo, "-c", "core.autocrlf=false", "add", "crlf.py")
        git(erosion_repo, "commit", "-q", "-m", "crlf", "-m", "A CRLF file for a review test.")
        (erosion_repo / "crlf.py").write_bytes(b"x = 1\r\ny = 3\r\n")
        seen = []
        real = erosion.complexity_delta

        def spy(pairs, threshold):
            seen.extend(pairs)
            return real(pairs, threshold)

        monkeypatch.setattr(erosion, "complexity_delta", spy)

        review(service, erosion_repo)

        assert seen == [erosion.TextPair("crlf.py", "x = 1\r\ny = 2\r\n", "x = 1\r\ny = 3\r\n")]

    def test_a_review_leaves_the_index_untouched(self, service, erosion_repo):
        (erosion_repo / "app.py").write_text(function("handle", 11))
        index = erosion_repo / ".git" / "index"
        before = (index.read_bytes(), index.stat().st_mtime_ns)

        review(service, erosion_repo)

        assert (index.read_bytes(), index.stat().st_mtime_ns) == before


def runner_failing(subcommand, outcome, when=lambda _arguments: True):
    """A git runner that answers ``subcommand`` with ``outcome`` and runs everything else."""
    calls = []

    def runner(cwd, arguments, *, timeout, max_output_bytes):
        calls.append((tuple(arguments), timeout, max_output_bytes))
        if arguments[0] == subcommand and when(arguments):
            return outcome
        return gitinfo.run_bounded(
            cwd, arguments, timeout=timeout, max_output_bytes=max_output_bytes
        )

    runner.calls = calls
    return runner


class TestReviewModeGitFailures:
    """Every git call is bounded, and every failure is a visible row."""

    def reviewed(self, extractor, root, runner):
        return review(LintService(extractor, runner), root)

    def test_every_git_call_carries_a_timeout_and_an_output_cap(self, extractor, erosion_repo):
        (erosion_repo / "app.py").write_text(function("handle", 11))
        runner = runner_failing("none", None)

        self.reviewed(extractor, erosion_repo, runner)

        assert [call[0][0] for call in runner.calls] == [
            "rev-parse",
            "diff",
            "ls-files",
            "cat-file",
        ]
        assert all(timeout > 0 and cap > 0 for _arguments, timeout, cap in runner.calls)

    def test_a_revision_git_could_not_run_for_is_one_row(self, extractor, erosion_repo):
        outcome = GitOutcome(None, "git is not installed, so repository state is unknown")

        row = only_row(self.reviewed(extractor, erosion_repo, runner_failing("rev-parse", outcome)))

        assert row.message.startswith("not checked: git did not resolve --since HEAD")
        assert row.evidence == outcome.note

    def test_a_revision_git_printed_junk_for_is_refused(self, extractor, erosion_repo):
        outcome = GitOutcome(b"junk\n", "", returncode=0)

        with pytest.raises(OperationFailed, match="did not print one full object name"):
            self.reviewed(extractor, erosion_repo, runner_failing("rev-parse", outcome))

    @pytest.mark.parametrize("subcommand", ["diff", "ls-files"])
    def test_a_listing_that_failed_is_one_row(self, extractor, erosion_repo, subcommand):
        (erosion_repo / "app.py").write_text(function("handle", 11))
        outcome = GitOutcome(None, f"git {subcommand} timed out after 30.0s")

        row = only_row(self.reviewed(extractor, erosion_repo, runner_failing(subcommand, outcome)))

        assert row.message.startswith(
            f"not checked: git {subcommand} did not list the changed files"
        )
        assert row.evidence == outcome.note

    def test_an_unmerged_path_is_one_row(self, extractor, erosion_repo):
        listing = b":000000 000000 0000000 0000000 U\0app.py\0"
        runner = runner_failing("diff", GitOutcome(listing, "", returncode=0))

        row = only_row(self.reviewed(extractor, erosion_repo, runner))

        assert (row.location, row.evidence) == ("app.py", "status U")
        assert "with status U" in row.message

    def test_a_listing_over_the_cap_is_one_row(self, extractor, erosion_repo):
        (erosion_repo / "app.py").write_text(function("handle", 11))
        outcome = GitOutcome(b":100644", "", truncated=True, returncode=0)

        row = only_row(self.reviewed(extractor, erosion_repo, runner_failing("diff", outcome)))

        assert "printed more than 8000000 bytes" in row.message

    def test_one_blob_that_failed_costs_only_its_file(self, extractor, make_git_repo):
        root = make_git_repo(
            {"a.py": function("first", 10), "b.py": function("second", 10)}, name="blob-fail"
        )
        (root / "a.py").write_text(function("first", 11))
        (root / "b.py").write_text(function("second", 11))
        note = "git cat-file exited 128: fatal: bad object"
        runner = runner_failing(
            "cat-file",
            GitOutcome(None, note, returncode=128),
            lambda arguments: "a.py" in arguments[2],
        )

        gap, found = self.reviewed(extractor, root, runner).findings

        assert (gap.location, gap.check) == ("a.py", patchlint.CHECK_COVERAGE)
        assert gap.message == (
            f"not checked: the text of a.py before the change was not read ({note}), so no pair "
            "check compared this file."
        )
        assert (found.location, found.check) == ("b.py:1", erosion.CHECK_COMPLEXITY_DELTA)

    def test_a_blob_over_the_cap_is_a_coverage_row(self, extractor, erosion_repo):
        (erosion_repo / "app.py").write_text(function("handle", 11))
        outcome = GitOutcome(b"def ", "", truncated=True, returncode=0)

        row = only_row(self.reviewed(extractor, erosion_repo, runner_failing("cat-file", outcome)))

        assert (
            row.evidence == "skipped: the committed text exceeds the per-file cap of 1000000 bytes"
        )


CLONE_ONLY = (erosion.CHECK_CLONE_DELTA,)


def summed(name):
    """A five-line function; two of them with different names are Type-2 copies."""
    return (
        f"def {name}(items):\n"
        "    out = 0\n"
        "    for item in items:\n"
        "        out += item\n"
        "    return out\n"
    )


def clone_rows(findings):
    return [row for row in findings if row.check == erosion.CHECK_CLONE_DELTA]


@pytest.fixture
def clone_repo(make_git_repo):
    """A committed repository with one function, and a copy of it in each out-of-scope tree."""
    return make_git_repo(
        {
            "app.py": summed("total"),
            "tests/test_app.py": summed("tested"),
            "fixtures/sample.py": summed("sampled"),
            "README.md": "# x\n",
        },
        name="clones",
    )


def add_file(path, text):
    return block(path, "", text)


class TestCloneDeltaInTheService:
    def test_a_run_without_clone_delta_never_builds_the_index(
        self, service, clone_repo, monkeypatch, tmp_path
    ):
        def forbidden(*_arguments, **_keywords):
            message = "a run without clone_delta must not read the repository for it"
            raise AssertionError(message)

        monkeypatch.setattr(lint_service, "_clone_index", forbidden)
        ctx = resolve_repo(clone_repo, None)
        after = tmp_path / "after.py"
        after.write_text(summed("again"))

        without = tuple(name for name in ALL if name != erosion.CHECK_CLONE_DELTA)
        candidate = findings_of(service, clone_repo, add_file("lib.py", summed("again")), without)
        proposed = service.lint_proposed(ctx, "lib.py", after).candidates[0]
        reviewed = service.lint_revision(ctx, "HEAD").candidates[0]

        assert clone_rows([*candidate, *proposed.findings, *reviewed.findings]) == []

    def test_clone_delta_alone_skips_the_scan(self, service, clone_repo, monkeypatch):
        def forbidden(*_arguments, **_keywords):
            message = "a pair-only run must not reach this"
            raise AssertionError(message)

        monkeypatch.setattr(LintService, "_facts", forbidden)
        monkeypatch.setattr(patchlint, "lint_patch", forbidden)

        findings = findings_of(service, clone_repo, add_file("lib.py", summed("again")), CLONE_ONLY)

        (row,) = findings
        assert (row.check, row.severity, row.location) == (
            erosion.CHECK_CLONE_DELTA,
            "advisory",
            "lib.py:1",
        )
        assert row.evidence == "also at app.py:1"
        assert row.detail is None

    def test_the_default_run_reports_a_copy_after_the_patch_rows(self, service, clone_repo):
        findings = findings_of(service, clone_repo, add_file("lib.py", summed("again")), ALL)

        assert checks_in(findings)[-1] == erosion.CHECK_CLONE_DELTA
        assert len(clone_rows(findings)) == 1

    def test_a_copy_under_a_test_or_fixture_path_gives_no_row(self, service, clone_repo):
        text = add_file("tests/test_more.py", summed("more")) + add_file(
            "fixtures/more.py", summed("more")
        )

        assert findings_of(service, clone_repo, text, CLONE_ONLY) == ()

    def test_a_twin_only_under_a_test_or_fixture_path_is_not_seen(self, service, make_git_repo):
        root = make_git_repo(
            {"tests/test_app.py": summed("tested"), "fixtures/sample.py": summed("sampled")},
            name="twins-out-of-scope",
        )

        assert findings_of(service, root, add_file("lib.py", summed("again")), CLONE_ONLY) == ()

    def test_an_unreadable_repository_file_is_one_row(self, service, clone_repo):
        (clone_repo / "latin.py").write_bytes(b"y = 1\nx = '\xff'\n")

        findings = findings_of(service, clone_repo, add_file("lib.py", summed("again")), CLONE_ONLY)

        (row,) = [row for row in findings if row.severity == "not_checked"]
        assert row.check == erosion.CHECK_CLONE_DELTA
        assert row.message.endswith(
            "of 2 repository files, so it does not see a copy of a block in them: latin.py."
        )
        assert row.evidence == "first: latin.py: not UTF-8 text: byte 11 is invalid start byte"
        assert len(clone_rows(findings)) == 2

    def test_a_copy_near_duplicates_reports_is_not_reported_twice(self, service, make_git_repo):
        root = make_git_repo({"app.py": summed("total"), "lib.py": "VALUE = 1\n"}, name="overlap")
        text = block("lib.py", "VALUE = 1\n", "VALUE = 1\n\n\n" + summed("again"))
        checks = (patchlint.CHECK_NEAR_DUPLICATES, erosion.CHECK_CLONE_DELTA)

        findings = findings_of(service, root, text, checks)

        assert [(row.check, row.location) for row in findings if row.severity == "advisory"] == [
            (patchlint.CHECK_NEAR_DUPLICATES, "lib.py:4")
        ]

    def test_a_created_file_is_the_documented_overlap_gap(self, service, clone_repo):
        # near_duplicates reports line 0 for a created file, so both checks report it.
        checks = (patchlint.CHECK_NEAR_DUPLICATES, erosion.CHECK_CLONE_DELTA)

        findings = findings_of(service, clone_repo, add_file("lib.py", summed("again")), checks)

        assert [(row.check, row.line) for row in findings if row.severity == "advisory"] == [
            (patchlint.CHECK_NEAR_DUPLICATES, 0),
            (erosion.CHECK_CLONE_DELTA, 1),
        ]

    def test_without_near_duplicates_the_exact_copy_is_a_clone_finding(self, service, clone_repo):
        findings = findings_of(service, clone_repo, add_file("lib.py", summed("total")), CLONE_ONLY)

        assert [row.location for row in clone_rows(findings)] == ["lib.py:1"]

    def test_the_hook_form_runs_clone_delta_when_named(self, service, clone_repo, tmp_path):
        after = tmp_path / "after.py"
        after.write_text(summed("again"))
        ctx = resolve_repo(clone_repo, None)

        (entry,) = service.lint_proposed(ctx, "lib.py", after, CLONE_ONLY).candidates
        (default,) = service.lint_proposed(ctx, "lib.py", after).candidates

        assert [row.location for row in entry.findings] == ["lib.py:1"]
        assert clone_rows(default.findings) == []

    def test_review_mode_reports_a_new_copy(self, service, clone_repo):
        (clone_repo / "lib.py").write_text(summed("again"))

        entry = review(service, clone_repo, checks=CLONE_ONLY)

        assert [row.location for row in entry.findings] == ["lib.py:1"]
        assert clone_rows(review(service, clone_repo).findings) == []

    def test_review_mode_does_not_report_a_cross_file_move(self, service, make_git_repo):
        root = make_git_repo(
            {"a.py": summed("total"), "b.py": summed("again") + "\n\nVALUE = 1\n"},
            name="clone-move",
        )
        (root / "b.py").write_text("VALUE = 1\n")
        (root / "c.py").write_text(summed("again"))
        commit(root)
        (root / "d.py").write_text("x = 1\n")

        assert review(service, root, "HEAD~1", checks=CLONE_ONLY).findings == ()
