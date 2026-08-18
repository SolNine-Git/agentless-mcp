"""The write surface end to end: parse, check, apply, normalise.

These are the tests that hold the security posture in place. Every path a
patch names is checked against the repository root before anything opens it,
and applying a patch does not write to the caller's checkout unless they asked
for ``--in-place`` on a clean tree. Both are asserted on the observable
outcome -- a refusal with its type, a checkout whose status and HEAD did not
move -- rather than on which internal function was called.
"""

import json
import subprocess

import pytest

from agentless_mcp.application.patch_service import PatchService, load_edits
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import sandbox
from agentless_mcp.core.normalize import file_key
from agentless_mcp.core.patches import EditStatus, parse_blocks
from agentless_mcp.util.errors import AtlasError, SecurityRefusal

APP = """\
def add(left, right):
    # Sum them.
    return left + right


def subtract(left, right):
    return left - right
"""

UTIL = 'VERSION = "1"\n'

PATCH = """\
```python
### app.py
<<<<<<< SEARCH
    return left + right
=======
    return round(left + right, 2)
>>>>>>> REPLACE
```
"""

MULTI_FILE_PATCH = """\
### app.py
<<<<<<< SEARCH
    return left + right
=======
    return round(left + right, 2)
>>>>>>> REPLACE

### util.py
<<<<<<< SEARCH
VERSION = "1"
=======
VERSION = "2"
>>>>>>> REPLACE
"""

BREAKING_PATCH = """\
### app.py
<<<<<<< SEARCH
    return left + right
=======
    return round(left + right,
>>>>>>> REPLACE
"""

ESCAPE_PATCH = """\
### ../outside.py
<<<<<<< SEARCH
secret
=======
leaked
>>>>>>> REPLACE
"""

ABSOLUTE_PATCH = """\
### /etc/passwd
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
def ctx(repo):
    """The repository context one call is about."""
    return resolve_repo(repo, None)


@pytest.fixture
def service(extractor):
    """The write-side service, wired as bootstrap wires it."""
    return PatchService(extractor)


def git(root, *arguments):
    """Run one git command and return its stdout."""
    return subprocess.run(
        ["git", "-C", str(root), *arguments],
        check=True,
        capture_output=True,
        timeout=30,
    ).stdout.decode()


def edits_of(text):
    """Parse patch text into edits, failing the test on a malformed block."""
    result = parse_blocks(text)
    assert result.errors == ()
    return result.edits


class TestSecurity:
    def test_a_relative_escape_is_refused(self, service, ctx):
        with pytest.raises(SecurityRefusal, match="outside the root"):
            service.check(edits_of(ESCAPE_PATCH), ctx)

    def test_an_absolute_path_outside_the_root_is_refused(self, service, ctx):
        with pytest.raises(SecurityRefusal, match="outside the root"):
            service.check(edits_of(ABSOLUTE_PATCH), ctx)

    def test_the_refusal_never_echoes_the_raw_argument(self, service, ctx):
        with pytest.raises(SecurityRefusal) as caught:
            service.check(edits_of(ESCAPE_PATCH), ctx)
        assert "../outside.py" not in str(caught.value)

    def test_apply_refuses_before_a_worktree_is_ever_created(self, service, ctx):
        with pytest.raises(SecurityRefusal):
            service.apply(edits_of(ESCAPE_PATCH), ctx)
        assert not sandbox.scratch_root().exists()

    def test_two_spellings_of_one_path_are_one_file(self, service, ctx):
        text = MULTI_FILE_PATCH.replace("### app.py", "### ./app.py")
        report = service.check(edits_of(text), ctx)
        assert {check.path for check in report.files} == {"app.py", "util.py"}


class TestCheck:
    def test_a_clean_patch_checks_ok(self, service, ctx):
        report = service.check(edits_of(PATCH), ctx)
        assert report.ok
        assert [check.path for check in report.files] == ["app.py"]
        assert report.files[0].verdict is not None
        assert report.files[0].verdict.language == "python"

    def test_a_syntax_breaking_patch_is_caught(self, service, ctx):
        report = service.check(edits_of(BREAKING_PATCH), ctx)
        assert not report.ok
        assert report.files[0].verdict is not None
        assert report.files[0].verdict.new_errors > 0

    def test_a_patch_naming_a_missing_file_reports_it(self, service, ctx):
        text = PATCH.replace("### app.py", "### nowhere.py")
        report = service.check(edits_of(text), ctx)
        assert not report.ok
        assert report.result.outcomes[0].status is EditStatus.NO_SUCH_FILE

    def test_check_does_not_write_to_the_checkout(self, service, ctx, repo):
        before = git(repo, "status", "--porcelain")
        service.check(edits_of(PATCH), ctx)
        assert git(repo, "status", "--porcelain") == before
        assert (repo / "app.py").read_text(encoding="utf-8") == APP


class TestApply:
    def test_it_returns_a_unified_diff(self, service, ctx):
        report = service.apply(edits_of(PATCH), ctx)
        assert report.ok
        assert "--- a/app.py" in report.diff
        assert "-    return left + right" in report.diff
        assert "+    return round(left + right, 2)" in report.diff

    def test_it_leaves_the_checkout_bit_identical(self, service, ctx, repo):
        before_status = git(repo, "status", "--porcelain")
        before_head = git(repo, "rev-parse", "HEAD")

        service.apply(edits_of(MULTI_FILE_PATCH), ctx)

        assert git(repo, "status", "--porcelain") == before_status
        assert git(repo, "rev-parse", "HEAD") == before_head
        assert (repo / "app.py").read_text(encoding="utf-8") == APP
        assert (repo / "util.py").read_text(encoding="utf-8") == UTIL

    def test_a_multi_file_patch_diffs_both_files(self, service, ctx):
        report = service.apply(edits_of(MULTI_FILE_PATCH), ctx)
        assert report.ok
        assert "a/app.py" in report.diff
        assert "a/util.py" in report.diff

    def test_a_failed_edit_still_returns_the_partial_diff(self, service, ctx):
        text = MULTI_FILE_PATCH.replace('VERSION = "1"', "NOT PRESENT ANYWHERE", 1)
        report = service.apply(edits_of(text), ctx)
        assert not report.ok
        assert "a/app.py" in report.diff
        assert "a/util.py" not in report.diff
        assert report.result.failures[0].status is EditStatus.NOT_FOUND


class TestApplyInPlace:
    def test_a_dirty_tree_is_refused_with_the_count(self, service, repo):
        (repo / "app.py").write_text(APP + "\n# scratch\n", encoding="utf-8")
        dirty = resolve_repo(repo, None)
        with pytest.raises(AtlasError, match="1 files are modified"):
            service.apply(edits_of(PATCH), dirty, in_place=True)

    def test_a_clean_tree_is_written_and_diffed(self, service, ctx, repo):
        report = service.apply(edits_of(PATCH), ctx, in_place=True)
        assert report.ok
        assert "round(left + right, 2)" in (repo / "app.py").read_text(encoding="utf-8")
        assert report.diff == git(repo, "diff", "--no-color", "--no-ext-diff")

    def test_a_repository_with_unknown_state_is_refused(self, service, tmp_path):
        plain = tmp_path / "plain"
        plain.mkdir()
        (plain / "app.py").write_text(APP, encoding="utf-8")
        outside = resolve_repo(plain, None)
        with pytest.raises(AtlasError, match="could not be read"):
            service.apply(edits_of(PATCH), outside, in_place=True)


class TestNormalize:
    def test_two_spellings_of_one_change_share_a_key(self, service, ctx):
        spaced = PATCH.replace(
            "    return round(left + right, 2)", "    return round(left+right,  2)"
        )
        assert (
            service.normalize(edits_of(PATCH), ctx).key
            == service.normalize(edits_of(spaced), ctx).key
        )

    def test_a_comment_only_patch_hashes_as_no_change(self, service, ctx):
        commented = """\
### app.py
<<<<<<< SEARCH
    # Sum them.
=======
    # Sum the two numbers together.
>>>>>>> REPLACE
"""
        report = service.normalize(edits_of(commented), ctx)
        assert report.ok
        assert report.file_keys["app.py"] == file_key(APP, APP, "python")
        assert report.key != service.normalize(edits_of(PATCH), ctx).key

    def test_a_different_change_hashes_differently(self, service, ctx):
        other = PATCH.replace("round(left + right, 2)", "left * right")
        assert (
            service.normalize(edits_of(PATCH), ctx).key
            != service.normalize(edits_of(other), ctx).key
        )

    def test_the_key_covers_every_edited_file(self, service, ctx):
        report = service.normalize(edits_of(MULTI_FILE_PATCH), ctx)
        assert set(report.file_keys) == {"app.py", "util.py"}


class TestLoadEdits:
    def test_raw_blocks_round_trip_through_the_json_form(self):
        parsed = parse_blocks(MULTI_FILE_PATCH)
        reloaded = load_edits(json.dumps(parsed.as_dict()))
        assert reloaded.edits == parsed.edits

    def test_a_json_document_missing_a_field_is_refused(self):
        with pytest.raises(AtlasError, match="missing a string 'search'"):
            load_edits('{"edits": [{"path": "a.py", "replace": "x"}]}')

    def test_a_document_without_an_edits_list_is_refused(self):
        with pytest.raises(AtlasError, match="'edits' list"):
            load_edits('{"blocks": []}')

    def test_invalid_json_is_refused_with_the_parse_error(self):
        with pytest.raises(AtlasError, match="not valid JSON"):
            load_edits('{"edits": [')
