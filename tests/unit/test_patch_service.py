"""The write surface end to end: parse, check, apply, normalise.

These are the tests that hold the security posture in place. Every path a
patch names is checked against the repository root before anything opens it,
and applying a patch does not write to the caller's checkout unless they asked
for ``--in-place`` on a clean tree. Both are asserted on the observable
outcome -- a refusal with its type, a checkout whose status and HEAD did not
move -- rather than on which internal function was called.
"""

import json
import os
import subprocess
from pathlib import Path

import pytest

from agentless_mcp.application import patch_service
from agentless_mcp.application.patch_service import PatchService, load_edits
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import sandbox
from agentless_mcp.core.normalize import file_key
from agentless_mcp.core.patches import EditStatus, parse_blocks
from agentless_mcp.util.errors import AgentlessError, SecurityRefusal

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

        with pytest.raises(AgentlessError, match=r"util\.py"):
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

        with pytest.raises(AgentlessError, match="every original was restored"):
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

        with pytest.raises(AgentlessError, match="every original was restored"):
            patch_service._write_all(
                tmp_path, {name: f"REWRITTEN = {index}\n" for index, name in enumerate(originals)}
            )

        assert {path.name for path in tmp_path.iterdir()} == set(originals)
        for name, content in originals.items():
            assert (tmp_path / name).read_text(encoding="utf-8") == content

    def test_a_rollback_that_also_fails_names_every_surviving_backup(self, repo, monkeypatch):
        """The one state that leaves a file missing from its own name.

        `_write_all` moves each original to its reserved backup before moving
        the staged replacement in. When a later replacement fails and the
        *restore* fails too, that file exists only as a backup sibling, and
        the message naming those paths is the whole recovery procedure. The
        adjacent test exercises the successful rollback; this is the branch
        the three-phase design exists for, and it had no test at all.
        """
        real = Path.replace
        staged = 0

        def fail_the_second_move_and_every_restore(source, destination):
            nonlocal staged
            if source.name.endswith(patch_service.BACKUP_SUFFIX):
                message = "simulated restore failure"
                raise OSError(message)
            if source.name.endswith(patch_service.STAGING_SUFFIX):
                staged += 1
                if staged == 2:
                    message = "simulated replace failure"
                    raise OSError(message)
            return real(source, destination)

        monkeypatch.setattr(Path, "replace", fail_the_second_move_and_every_restore)

        with pytest.raises(AgentlessError) as caught:
            patch_service._write_all(repo, {"app.py": "new app\n", "util.py": "new util\n"})

        message = str(caught.value)
        assert "patch partly applied: cannot replace util.py" in message
        assert "rollback failed; originals retained at:" in message

        backups = {
            path.name.split(".")[1]: path for path in repo.glob(f"*{patch_service.BACKUP_SUFFIX}")
        }
        assert set(backups) == {"app", "util"}
        # util.py is not at its own name any more: it is only the backup.
        assert not (repo / "util.py").exists()
        assert backups["util"].read_text(encoding="utf-8") == UTIL
        assert (repo / "app.py").read_text(encoding="utf-8") == "new app\n"
        assert backups["app"].read_text(encoding="utf-8") == APP
        # Restored in reverse order, so the message names them that way.
        assert message.index(str(backups["util"])) < message.index(str(backups["app"]))
        # A failed staging move leaves no staging file behind either.
        assert not list(repo.glob(f"*{patch_service.STAGING_SUFFIX}"))

    def test_a_staging_write_that_fails_leaves_no_staging_file(
        self, service, ctx, repo, monkeypatch
    ):
        """Fault-injected below `_stage_file`, so its own cleanup runs.

        The neighbouring test monkeypatches `_stage_file` wholesale, which
        proves the outer contract and never enters the resource-safety code
        inside it.
        """

        def refuse(descriptor, content, target_name):
            _ = (descriptor, content)
            message = "no space left on device"
            raise OSError(28, message, target_name)

        monkeypatch.setattr(patch_service, "_write_descriptor", refuse)

        with pytest.raises(AgentlessError, match=r"cannot write app\.py"):
            service.apply(edits_of(MULTI_FILE_PATCH), ctx, in_place=True)

        assert not list(repo.glob(f"*{patch_service.STAGING_SUFFIX}"))
        assert (repo / "app.py").read_text(encoding="utf-8") == APP
        assert (repo / "util.py").read_text(encoding="utf-8") == UTIL

    def test_an_interrupt_mid_write_still_removes_the_staging_file(self, repo, monkeypatch):
        """`except BaseException` rather than `except Exception`, pinned.

        A KeyboardInterrupt is the case the broader clause exists for: an
        `except Exception` would let it past and leave both the descriptor and
        the staging file behind.
        """

        def interrupt(descriptor, content, target_name):
            _ = (descriptor, content, target_name)
            raise KeyboardInterrupt

        monkeypatch.setattr(patch_service, "_write_descriptor", interrupt)

        with pytest.raises(KeyboardInterrupt):
            patch_service._stage_file(repo / "app.py", "new app\n")

        assert not list(repo.glob(f"*{patch_service.STAGING_SUFFIX}"))

    def test_a_backup_that_cannot_be_reserved_changes_nothing(
        self, service, ctx, repo, monkeypatch
    ):
        """The second phase's own failure: nothing is moved, nothing is left."""

        def refuse(target, suffix):
            _ = suffix
            message = "read-only file system"
            raise OSError(30, message, str(target))

        monkeypatch.setattr(patch_service, "_reserve_sibling", refuse)

        with pytest.raises(AgentlessError, match="cannot reserve rollback files"):
            service.apply(edits_of(MULTI_FILE_PATCH), ctx, in_place=True)

        assert (repo / "app.py").read_text(encoding="utf-8") == APP
        assert (repo / "util.py").read_text(encoding="utf-8") == UTIL
        assert not list(repo.glob(f"*{patch_service.STAGING_SUFFIX}"))
        assert not list(repo.glob(f"*{patch_service.BACKUP_SUFFIX}"))

    def test_a_short_write_is_an_error_rather_than_a_truncated_file(self, tmp_path, monkeypatch):
        """`os.write` may write fewer bytes than it was given, and returning
        zero forever is the case the loop cannot make progress against."""
        descriptor = os.open(tmp_path / "sink.txt", os.O_WRONLY | os.O_CREAT, 0o600)
        real_write = os.write

        def write_nothing(fileno, data):
            # Only this descriptor: pytest captures output through os.write.
            return 0 if fileno == descriptor else real_write(fileno, data)

        monkeypatch.setattr(os, "write", write_nothing)
        try:
            with pytest.raises(OSError, match=r"write returned zero bytes for app\.py"):
                patch_service._write_descriptor(descriptor, b"content", "app.py")
        finally:
            os.close(descriptor)

    def test_a_file_that_genuinely_holds_u_fffd_is_still_editable(self, service, repo):
        """The strict decode decides, so a real U+FFFD is not a lossy decode.

        `read_bounded` maps every undecodable byte to U+FFFD, which is why the
        write side treats the character as the sign of a lossy read -- and why
        it then has to check the bytes rather than trust the sign. A file that
        contains the character legitimately must stay editable, and its bytes
        must come back unchanged.
        """
        original = 'VERSION = "1"  # \ufffd marker\n'
        patched = 'VERSION = "2"  # \ufffd marker\n'
        (repo / "util.py").write_text(original, encoding="utf-8")
        git(repo, "commit", "-am", "a real replacement character")
        clean = resolve_repo(repo, None)
        text = f"### util.py\n<<<<<<< SEARCH\n{original}=======\n{patched}>>>>>>> REPLACE\n"

        report = service.apply(edits_of(text), clean, in_place=True)

        assert report.ok, [outcome.reason for outcome in report.result.outcomes]
        assert (repo / "util.py").read_bytes() == patched.encode("utf-8")

    def test_a_file_whose_bytes_cannot_be_reread_is_reported_unreadable(
        self, service, ctx, repo, monkeypatch
    ):
        """The strict re-read is IO, so it has its own failure to report."""
        (repo / "app.py").write_text(APP + "# \ufffd\n", encoding="utf-8")
        git(repo, "commit", "-am", "a real replacement character")
        clean = resolve_repo(repo, None)
        _ = ctx
        real = Path.read_bytes

        def refuse(path):
            if path.name == "app.py":
                message = "permission denied"
                raise OSError(13, message, str(path))
            return real(path)

        monkeypatch.setattr(Path, "read_bytes", refuse)

        report = service.apply(edits_of(PATCH), clean, in_place=True)

        assert not report.ok
        assert "unreadable: permission denied" in report.result.outcomes[0].reason

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
        with pytest.raises(AgentlessError, match="1 files are modified"):
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
        with pytest.raises(AgentlessError, match="could not be read"):
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
        with pytest.raises(AgentlessError, match="missing a string 'search'"):
            load_edits('{"edits": [{"path": "a.py", "replace": "x"}]}')

    def test_a_document_without_an_edits_list_is_refused(self):
        with pytest.raises(AgentlessError, match="'edits' list"):
            load_edits('{"blocks": []}')

    def test_invalid_json_is_refused_with_the_parse_error(self):
        with pytest.raises(AgentlessError, match="not valid JSON"):
            load_edits('{"edits": [')

    def test_an_edits_field_that_is_not_a_list_is_refused(self):
        with pytest.raises(AgentlessError, match="must be a list of edit objects"):
            load_edits('{"edits": {"path": "a.py"}}')

    def test_a_list_entry_that_is_not_an_object_is_refused(self):
        with pytest.raises(AgentlessError, match="edit 0 is not a JSON object"):
            load_edits('{"edits": ["### a.py"]}')

    def test_a_non_integer_index_is_refused(self):
        document = '{"edits": [{"path": "a.py", "search": "x", "replace": "y", "index": "1"}]}'
        with pytest.raises(AgentlessError, match="edit 0 has a non-integer 'index'"):
            load_edits(document)

    def test_an_empty_search_never_reaches_the_write(self, service, ctx, repo):
        """An empty pre-image matches nothing; it used to append at end of file.

        Refused in `core.patches._apply_one`, the one function in the package
        that can write, so both the JSON form and raw blocks reach the same
        answer. Pinned from the service because the write is what it costs.
        """
        parsed = load_edits('{"edits": [{"path": "app.py", "search": "", "replace": "X"}]}')

        report = service.apply(parsed.edits, ctx, in_place=True)

        assert not report.ok
        assert "the SEARCH side is empty" in report.result.outcomes[0].reason
        assert (repo / "app.py").read_text(encoding="utf-8") == APP
