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
from pathlib import Path

import pytest

from agentless_mcp.application import patch_service
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

GIT_IDENTITY = ("-c", "user.email=tests@example.invalid", "-c", "user.name=agentless-mcp tests")


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
        ["git", *GIT_IDENTITY, "-C", str(root), *arguments],
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

    def test_two_edits_naming_one_file_both_land(self, service, ctx):
        """The ordinary shape of a real patch, and it reached no test before.

        ``_read`` deduplicates by path so the file is read once, and the
        edits then run against one accumulating copy. Two independent hunks
        in one file is what an agent produces most of the time.
        """
        text = (
            "### app.py\n<<<<<<< SEARCH\n    return left + right\n"
            "=======\n    return left + right + 0\n>>>>>>> REPLACE\n"
            "### app.py\n<<<<<<< SEARCH\n    return left - right\n"
            "=======\n    return left - right - 0\n>>>>>>> REPLACE\n"
        )
        report = service.apply(edits_of(text), ctx)

        assert report.ok
        assert len(report.result.outcomes) == 2
        assert "+    return left + right + 0" in report.diff
        assert "+    return left - right - 0" in report.diff

    def test_a_later_edit_sees_what_an_earlier_one_wrote(self, service, ctx):
        """Order is meaningful within a file, so it has to be pinned.

        The second block below searches for text that only exists because the
        first block created it. If the edits ran against independent copies of
        the original, the second would report `not_found`.
        """
        text = (
            "### app.py\n<<<<<<< SEARCH\n    return left + right\n"
            "=======\n    return MARKER\n>>>>>>> REPLACE\n"
            "### app.py\n<<<<<<< SEARCH\n    return MARKER\n"
            "=======\n    return left + right + 1\n>>>>>>> REPLACE\n"
        )
        report = service.apply(edits_of(text), ctx)

        assert report.ok
        assert "MARKER" not in report.diff
        assert "+    return left + right + 1" in report.diff

    def test_a_missing_file_is_reported_as_missing_not_as_unreadable(self, service, ctx):
        """``apply`` names the same cause ``check`` does, for the same file."""
        text = PATCH.replace("### app.py", "### nowhere.py")
        report = service.apply(edits_of(text), ctx)
        assert report.result.outcomes[0].status is EditStatus.NO_SUCH_FILE
        assert "nowhere.py" in report.result.outcomes[0].reason

    def test_a_failed_edit_writes_nothing_at_all(self, service, ctx):
        """Half a patch is not a patch: a failed sibling cancels the whole write.

        The edits that *did* match are still in ``new_contents``, so writing
        them left the tree holding an arbitrary prefix of the patch with
        ``ok`` false and nothing saying which prefix.
        """
        text = MULTI_FILE_PATCH.replace('VERSION = "1"', "NOT PRESENT ANYWHERE", 1)
        report = service.apply(edits_of(text), ctx)
        assert not report.ok
        assert report.diff == ""
        assert report.result.failures[0].status is EditStatus.NOT_FOUND


class TestApplyInPlace:
    def test_a_non_utf8_file_is_refused_rather_than_rewritten(self, service, repo):
        """The lossy analysis read must never become the bytes written back.

        ``read_bounded`` decodes with ``errors="replace"``, which is right for
        a repository scan and fatal for a round trip: writing the decoded
        string back turns every undecodable byte into U+FFFD, including in
        regions no edit named, while the report still says ok.
        """
        latin1 = APP.encode("utf-8") + b"# tail \xe9\n"
        (repo / "app.py").write_bytes(latin1)
        git(repo, "commit", "-am", "latin-1 tail")
        dirty = resolve_repo(repo, None)

        report = service.apply(edits_of(PATCH), dirty, in_place=True)

        assert not report.ok
        assert (repo / "app.py").read_bytes() == latin1
        assert "UTF-8" in report.result.outcomes[0].reason

    def test_a_write_that_fails_leaves_every_file_as_it_was(self, service, ctx, repo, monkeypatch):
        """An OSError mid-write must not leave an arbitrary prefix on disk."""
        real = patch_service._stage_file

        def refuse(target, content):
            if target.name == "util.py":
                message = "no space left on device"
                raise OSError(28, message)
            return real(target, content)

        monkeypatch.setattr(patch_service, "_stage_file", refuse)

        with pytest.raises(AtlasError, match=r"util\.py"):
            service.apply(edits_of(MULTI_FILE_PATCH), ctx, in_place=True)

        assert (repo / "app.py").read_text(encoding="utf-8") == APP
        assert (repo / "util.py").read_text(encoding="utf-8") == UTIL

    def test_an_existing_staging_sibling_is_not_touched(self, repo):
        sibling = repo / f"app.py{patch_service.STAGING_SUFFIX}"
        sibling.write_text("user-owned\n", encoding="utf-8")

        patch_service._write_all(repo, {"app.py": APP.replace("left + right", "left - right")})

        assert sibling.read_text(encoding="utf-8") == "user-owned\n"

    def test_an_existing_staging_symlink_is_not_followed(self, repo):
        outside = repo.parent / "outside.txt"
        outside.write_text("outside\n", encoding="utf-8")
        sibling = repo / f"app.py{patch_service.STAGING_SUFFIX}"
        sibling.symlink_to(outside)

        patch_service._write_all(repo, {"app.py": APP.replace("left + right", "left - right")})

        assert outside.read_text(encoding="utf-8") == "outside\n"
        assert sibling.is_symlink()

    def test_a_replace_failure_restores_every_original(self, repo, monkeypatch):
        real = Path.replace
        replacements = 0

        def fail_second_staging(staging, target):
            nonlocal replacements
            if staging.name.endswith(patch_service.STAGING_SUFFIX):
                replacements += 1
                if replacements == 2:
                    message = "simulated replace failure"
                    raise OSError(message)
            return real(staging, target)

        monkeypatch.setattr(Path, "replace", fail_second_staging)

        with pytest.raises(AtlasError, match="every original was restored"):
            patch_service._write_all(repo, {"app.py": "new app\n", "util.py": "new util\n"})

        assert (repo / "app.py").read_text(encoding="utf-8") == APP
        assert (repo / "util.py").read_text(encoding="utf-8") == UTIL

    def test_a_failure_at_the_third_of_five_leaves_no_residue(self, tmp_path, monkeypatch):
        """Rollback has to undo a prefix and discard a suffix in one pass.

        The two-file case above proves the prefix restores. Five files with
        the failure in the middle is what proves the *suffix* is discarded
        too: staging siblings for files four and five were created before the
        third one failed, and a leftover `.agentless-mcp-staging` file in a
        checkout is a patch that half-happened.
        """
        originals = {f"m{index}.py": f"VALUE = {index}\n" for index in range(5)}
        for name, content in originals.items():
            (tmp_path / name).write_text(content, encoding="utf-8")

        real = Path.replace
        replacements = 0

        def fail_third_staging(staging, target):
            nonlocal replacements
            if staging.name.endswith(patch_service.STAGING_SUFFIX):
                replacements += 1
                if replacements == 3:
                    message = "simulated replace failure"
                    raise OSError(message)
            return real(staging, target)

        monkeypatch.setattr(Path, "replace", fail_third_staging)

        with pytest.raises(AtlasError, match="every original was restored"):
            patch_service._write_all(
                tmp_path, {name: f"REWRITTEN = {index}\n" for index, name in enumerate(originals)}
            )

        assert {path.name for path in tmp_path.iterdir()} == set(originals)
        for name, content in originals.items():
            assert (tmp_path / name).read_text(encoding="utf-8") == content

    def test_crlf_bytes_survive_the_write(self, service, repo):
        """The writer performs no newline translation, on any platform.

        ``write_text``'s default ``newline=None`` translates every ``\\n`` to
        ``os.linesep`` on write, so on Windows a patched LF file came back
        CRLF and a patched CRLF file came back ``\\r\\r\\n``. Bytes in, bytes
        out: this assertion is the same on every platform, which is the point.
        """
        crlf = b'VERSION = "1"\r\nBUILD = 7\r\n'
        (repo / "util.py").write_bytes(crlf)
        git(repo, "commit", "-am", "crlf util")
        dirty = resolve_repo(repo, None)
        text = "### util.py\n<<<<<<< SEARCH\nBUILD = 7\r\n=======\nBUILD = 8\r\n>>>>>>> REPLACE\n"

        report = service.apply(edits_of(text), dirty, in_place=True)

        assert report.ok, [outcome.reason for outcome in report.result.outcomes]
        assert (repo / "util.py").read_bytes() == b'VERSION = "1"\r\nBUILD = 8\r\n'

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
