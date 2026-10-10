"""The lint service's two coverage rules: it decides nothing and it survives.

``lint`` has no verdict and exits 0 whatever it finds, so every per-candidate
failure has to become a row in the report rather than the end of the run. The
two cases here are the ones that used to break that: a candidate naming a path
outside the repository, which took the whole run down with it, and a repository
file the reader skipped, which the report has to admit it never compared.
"""

import json

import pytest

from agentless_mcp.application import lint_service
from agentless_mcp.application.lint_service import LintCandidateInput, LintService
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import erosion, patchlint
from agentless_mcp.core.patches import parse_blocks
from agentless_mcp.util import fslimits
from agentless_mcp.util.errors import OperationFailed

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

    def test_complexity_delta_and_contract_drift_are_the_pair_checks(self):
        assert lint_service.PAIR_CHECK_NAMES == (
            erosion.CHECK_COMPLEXITY_DELTA,
            erosion.CHECK_CONTRACT_DRIFT,
        )

    @pytest.mark.parametrize(
        ("needs", "runner"),
        [
            (lint_service.CheckInput.PAIRS, None),
            (lint_service.CheckInput.PATCH, lambda _pairs, _config: ()),
        ],
        ids=["pair-check-without-runner", "patch-check-with-runner"],
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
