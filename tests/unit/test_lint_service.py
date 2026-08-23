"""The lint service's two coverage rules: it decides nothing and it survives.

``lint`` has no verdict and exits 0 whatever it finds, so every per-candidate
failure has to become a row in the report rather than the end of the run. The
two cases here are the ones that used to break that: a candidate naming a path
outside the repository, which took the whole run down with it, and a repository
file the reader skipped, which the report has to admit it never compared.
"""

import pytest

from agentless_mcp.application import lint_service
from agentless_mcp.application.lint_service import LintCandidateInput, LintService
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import patchlint
from agentless_mcp.core.patches import parse_blocks
from agentless_mcp.util import fslimits

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
