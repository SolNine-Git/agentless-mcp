"""The tag cache: incremental indexing, freshness, locking and equivalence.

Two properties carry this suite. **Reuse is keyed on content**, so the tests
count real extractions with a spy on the extractor rather than trusting a
report's own numbers. And **a cached answer is the same answer**, so the
equivalence tests compare whole rendered bodies from a fresh index against the
same command run with ``--no-cache``.

Every test runs with ``XDG_CACHE_HOME`` pointed at a throwaway directory by
the autouse fixture in ``tests/conftest.py``; nothing here can reach the
developer's real cache.
"""

import json
import logging
import sqlite3
import stat
import subprocess
import sys
import threading
import time
from contextlib import closing
from pathlib import Path

import pytest

from agentless_mcp.adapters.cli.formatting import EXIT_DOMAIN, EXIT_OK
from agentless_mcp.adapters.cli.main import CliServices, run
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.lint_service import LintService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.validate_service import ValidateService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import cache, gitinfo, grammars, patchlint, refs
from agentless_mcp.core.symbols import symbol_stable_id
from agentless_mcp.util import cachedir, filelock, fslimits
from agentless_mcp.util.errors import CacheLocked, OperationFailed

CORE = '''\
"""Core."""

RATE = 3


def quote(sku):
    return RATE


class PriceBook:
    def cost_of(self, sku):
        return quote(sku)
'''

BILLING = """\
from core import quote


def run_billing(items):
    return sum(quote(item) for item in items)
"""

# Parsed, supported, and defines nothing: the case that must still be recorded
# with its digest so the next run skips it instead of re-parsing it forever.
EMPTY = "# nothing here yet\n"


@pytest.fixture(autouse=True)
def close_open_sources(monkeypatch):
    """Make every source opened by this module request-scoped and explicit."""
    opened = []
    real = cache.open_source

    def tracked(*args, **kwargs):
        source = real(*args, **kwargs)
        opened.append(source)
        return source

    monkeypatch.setattr(cache, "open_source", tracked)
    yield
    for source in reversed(opened):
        source.close()


FIXTURE_REPOS = ("repo_py", "repo_ts", "repo_go")
SKELETON_FILES = {"repo_py": "pricing.py", "repo_ts": "pricing.ts", "repo_go": "pricing.go"}


@pytest.fixture
def repo(tmp_path):
    """A three-file repository with no git, one file of which defines nothing."""
    root = tmp_path / "plain"
    root.mkdir()
    (root / "core.py").write_text(CORE, encoding="utf-8")
    (root / "billing.py").write_text(BILLING, encoding="utf-8")
    (root / "empty.py").write_text(EMPTY, encoding="utf-8")
    return root


@pytest.fixture
def services(extractor, counter):
    """The same wiring bootstrap builds, without the console-script layer."""
    return CliServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor, counter),
        graphs=GraphService(extractor),
        patches=PatchService(extractor),
        validates=ValidateService(PatchService(extractor)),
        lints=LintService(extractor),
        counter=counter,
        extractor=extractor,
    )


@pytest.fixture
def spy(extractor, monkeypatch):
    """Record every path the extractor actually parses symbols out of."""
    parsed: list[str] = []
    original = extractor.extract_from_source

    def record(text, language, path):
        parsed.append(path)
        return original(text, language, path)

    monkeypatch.setattr(extractor, "extract_from_source", record)
    return parsed


@pytest.fixture
def parse_spy(extractor, monkeypatch):
    """Record every parse the extractor performs, by kind and by path.

    All three kinds, not just symbols: the cache stores symbols, imports and
    references now, so "one file touched, one re-parse" has to be provable for
    each of them separately. A cache that quietly re-parsed every file for its
    references would still pass a symbols-only spy.
    """
    parsed: dict[str, list[str]] = {"symbols": [], "imports": [], "refs": []}

    for kind, name in (
        ("symbols", "extract_from_source"),
        ("imports", "extract_imports_from_source"),
        ("refs", "extract_refs_from_source"),
    ):
        original = getattr(extractor, name)

        def record(text, language, path, _original=original, _kind=kind):
            parsed[_kind].append(path)
            return _original(text, language, path)

        monkeypatch.setattr(extractor, name, record)

    return parsed


def invoke(services, root, *arguments):
    """Run one CLI subcommand against ``root``."""
    return run([*arguments, "--repo", str(root)], services)


def body(output: str) -> str:
    """Drop the receipt line, whose cache field is what deliberately differs."""
    return "\n".join(line for line in output.splitlines() if not line.startswith("# repo:"))


def git(root, *arguments):
    """Run one git command in ``root``, failing loudly."""
    subprocess.run(
        ["git", "-C", str(root), *arguments], check=True, capture_output=True, timeout=30
    )


def commit(root, message):
    """Commit everything in ``root`` with a pinned identity."""
    git(root, "add", "-A")
    git(
        root,
        "-c",
        "user.email=tests@example.invalid",
        "-c",
        "user.name=agentless-mcp tests",
        "commit",
        "-m",
        message,
    )


class TestLocation:
    def test_the_database_lives_under_the_cache_home_and_not_in_the_repo(
        self, repo, extractor, isolated_cache_home
    ):
        report = cache.build_index(repo, extractor)

        assert report.database.exists()
        assert isolated_cache_home in report.database.parents
        assert not any(repo.rglob("*.db"))

    @pytest.mark.skipif(sys.platform.startswith("win"), reason="POSIX directory modes")
    def test_both_cache_directory_levels_are_owner_only(self, repo, extractor):
        """The claim has to hold on an upgraded install, not only a fresh one.

        ``mkdir(mode=...)`` sets the leaf and nothing else, and never
        re-applies the mode to a directory that already exists.
        """
        application = cachedir.cache_root()
        application.mkdir(parents=True, exist_ok=True)
        application.chmod(0o755)

        database = cache.build_index(repo, extractor).database

        assert stat.S_IMODE(application.stat().st_mode) == cachedir.DIRECTORY_MODE
        assert stat.S_IMODE(database.parent.stat().st_mode) == cachedir.DIRECTORY_MODE

    def test_two_repositories_get_two_databases(self, repo, tmp_path):
        other = tmp_path / "other"
        other.mkdir()

        assert cache.cache_path(repo) != cache.cache_path(other)


class TestIncrementalTriad:
    def test_a_second_run_reuses_every_unchanged_file(self, repo, extractor, spy):
        first = cache.build_index(repo, extractor)
        spy.clear()

        second = cache.build_index(repo, extractor)

        assert first.indexed == 3
        assert (second.indexed, second.reused, second.pruned) == (0, 3, 0)
        assert spy == []

    def test_touching_one_file_re_extracts_exactly_that_file(self, repo, extractor, spy):
        cache.build_index(repo, extractor)
        (repo / "core.py").write_text(CORE + "\n\ndef discount(sku):\n    return 1\n", "utf-8")
        spy.clear()

        report = cache.build_index(repo, extractor)

        assert spy == ["core.py"]
        assert (report.indexed, report.reused) == (1, 2)

    def test_a_file_defining_nothing_is_recorded_and_then_skipped(self, repo, extractor, spy):
        first = cache.build_index(repo, extractor)
        spy.clear()

        second = cache.build_index(repo, extractor)

        with closing(sqlite3.connect(first.database)) as connection:
            recorded = connection.execute(
                "SELECT COUNT(*) FROM files WHERE path = ?", ("empty.py",)
            ).fetchone()
            tagged = connection.execute(
                "SELECT COUNT(*) FROM tags WHERE path = ?", ("empty.py",)
            ).fetchone()

        assert (recorded[0], tagged[0]) == (1, 0)
        assert second.reused == 3
        assert spy == []

    def test_a_deleted_file_is_pruned(self, repo, extractor):
        cache.build_index(repo, extractor)
        (repo / "billing.py").unlink()

        report = cache.build_index(repo, extractor)

        assert report.pruned == 1
        assert report.files == 2
        with closing(sqlite3.connect(report.database)) as connection:
            rows = connection.execute(
                "SELECT COUNT(*) FROM tags WHERE path = ?", ("billing.py",)
            ).fetchone()
        assert rows[0] == 0

    def test_force_re_extracts_everything(self, repo, extractor, spy):
        cache.build_index(repo, extractor)
        spy.clear()

        report = cache.build_index(repo, extractor, force=True)

        assert sorted(spy) == ["billing.py", "core.py", "empty.py"]
        assert (report.indexed, report.reused) == (3, 0)

    def test_an_unreadable_file_is_reported_and_not_recorded(self, repo, extractor, monkeypatch):
        (repo / "locked.py").write_text(CORE, encoding="utf-8")
        monkeypatch.setattr(cache, "read_bounded", _refuse_one("locked.py"))

        report = cache.build_index(repo, extractor)

        assert report.errors == 1
        assert report.failures[0].path == "locked.py"
        assert "unreadable" in report.failures[0].reason

    def test_a_file_over_the_read_cap_is_a_skip_and_not_an_error(self, repo, extractor):
        """Declining to read a file is a decision, not a failure to read one.

        The reason the report carried already began "skipped:", so ``index``
        exited non-zero and named a file nobody had asked it to read.
        """
        oversize = repo / "huge.py"
        oversize.write_text("x = 1\n" * 200_000, encoding="utf-8")
        assert oversize.stat().st_size > fslimits.DEFAULT_MAX_FILE_BYTES

        report = cache.build_index(repo, extractor)

        assert report.errors == 0
        assert report.skipped == 1
        assert report.skipped_files[0].path == "huge.py"
        assert "per-file cap" in report.skipped_files[0].reason

    def test_a_file_the_extractor_trips_on_is_not_counted_as_pruned(
        self, repo, extractor, monkeypatch
    ):
        """``pruned`` means "files that left the repository" and nothing else.

        It was the complement of the paths the run recorded, and a failed
        extraction is removed from those, so one file was reported as both an
        error and a prune on the same summary line.
        """
        cache.build_index(repo, extractor)
        original = extractor.extract_from_source

        def explode(text, language, path):
            if path == "billing.py":
                message = "maximum recursion depth exceeded"
                raise RecursionError(message)
            return original(text, language, path)

        monkeypatch.setattr(extractor, "extract_from_source", explode)
        (repo / "billing.py").write_text(BILLING + "\nEXTRA = 1\n", encoding="utf-8")

        report = cache.build_index(repo, extractor)

        assert report.errors == 1
        assert report.pruned == 0

    def test_one_file_that_trips_the_extractor_does_not_abort_the_scan(
        self, repo, extractor, monkeypatch
    ):
        """A per-file extractor defect degrades that file, not the repository.

        ``RecursionError`` stands in for the whole degraded-error class: the
        scan catches a named tuple of them, so one pathological file becomes an
        ``IndexFailure`` row and every other file is still indexed.
        """
        original = extractor.extract_from_source

        def explode(text, language, path):
            if path == "billing.py":
                message = "maximum recursion depth exceeded"
                raise RecursionError(message)
            return original(text, language, path)

        monkeypatch.setattr(extractor, "extract_from_source", explode)

        report = cache.build_index(repo, extractor)

        assert report.errors == 1
        assert report.failures[0].path == "billing.py"
        assert "recursion" in report.failures[0].reason
        assert (report.indexed, report.files) == (2, 2)

    def test_an_unwarmed_language_is_a_skip_and_not_an_error(
        self, repo, extractor, spy, monkeypatch
    ):
        (repo / "helper.go").write_text("package main\n", encoding="utf-8")
        warmed = cache.grammars.warmed_languages()
        monkeypatch.setattr(cache.grammars, "warmed_languages", lambda: warmed - {"go"})

        report = cache.build_index(repo, extractor)

        assert report.errors == 0
        assert report.skipped == 1
        assert report.skipped_files[0].path == "helper.go"
        assert "not warmed" in report.skipped_files[0].reason
        assert (report.indexed, report.files) == (3, 4)
        assert "helper.go" not in spy

    def test_an_unwarmed_file_is_recorded_and_not_re_attempted(
        self, repo, extractor, spy, monkeypatch
    ):
        (repo / "helper.go").write_text("package main\n", encoding="utf-8")
        warmed = cache.grammars.warmed_languages()
        monkeypatch.setattr(cache.grammars, "warmed_languages", lambda: warmed - {"go"})
        first = cache.build_index(repo, extractor)
        spy.clear()

        second = cache.build_index(repo, extractor)

        with closing(sqlite3.connect(first.database)) as connection:
            stamped = connection.execute(
                "SELECT grammar_version FROM files WHERE path = ?", ("helper.go",)
            ).fetchone()
        assert stamped[0].startswith(cache.UNWARMED_STAMP_PREFIX)
        assert (second.indexed, second.reused, second.skipped) == (0, 3, 1)
        assert spy == []

    def test_warming_the_grammar_heals_a_skipped_file_without_force(
        self, repo, extractor, spy, monkeypatch
    ):
        (repo / "helper.go").write_text("package main\n", encoding="utf-8")
        warmed = cache.grammars.warmed_languages()
        with pytest.MonkeyPatch.context() as cold:
            cold.setattr(cache.grammars, "warmed_languages", lambda: warmed - {"go"})
            cache.build_index(repo, extractor)
        spy.clear()

        report = cache.build_index(repo, extractor)

        assert spy == ["helper.go"]
        assert (report.indexed, report.skipped) == (1, 0)

    @pytest.mark.parametrize(
        "error",
        [AttributeError, TypeError, KeyError, IndexError],
        ids=lambda error: error.__name__,
    )
    def test_an_extractor_programming_defect_surfaces(self, repo, extractor, monkeypatch, error):
        def defective(_text, _language, _path):
            message = "a renamed field"
            raise error(message)

        monkeypatch.setattr(extractor, "extract_from_source", defective)

        with pytest.raises(error, match="renamed field"):
            cache.build_index(repo, extractor)


def _refuse_one(name):
    """Wrap read_bounded so exactly one file reports itself unreadable."""
    original = fslimits.read_bounded

    def read(path, *arguments, **keywords):
        if path.name == name:
            return fslimits.BoundedRead(
                path=path, text=None, skipped="unreadable: Permission denied"
            )
        return original(path, *arguments, **keywords)

    return read


class TestFreshness:
    def test_no_database_at_all_reads_none(self, repo, extractor, services):
        source = cache.open_source(repo, extractor, tree_oid=None)

        assert source.receipt == "none"
        assert invoke(services, repo, "map") == EXIT_OK

    def test_a_fresh_index_names_its_generation(self, repo, extractor):
        report = cache.build_index(repo, extractor)

        source = cache.open_source(repo, extractor, tree_oid=None)

        assert source.receipt == f"g:{report.generation} fresh"

    def test_the_status_names_the_generation_comparison_once(self, repo, extractor):
        """One fact, one key. ``fresh`` and ``generation_matches`` were both it."""
        cache.build_index(repo, extractor)

        document = cache.open_source(repo, extractor, tree_oid=None).status().as_dict()

        assert document["generation_matches"] is True
        assert "fresh" not in document

    def test_no_cache_reports_that_it_was_bypassed(self, repo, extractor):
        cache.build_index(repo, extractor)

        source = cache.open_source(repo, extractor, tree_oid=None, no_cache=True)

        assert source.receipt == "bypassed (--no-cache)"

    def test_a_commit_reports_a_generation_mismatch_and_names_both_generations(
        self, make_git_repo, extractor, services, capsys
    ):
        root = make_git_repo({"core.py": CORE, "billing.py": BILLING})
        indexed = cache.build_index(root, extractor, tree_oid=_tree_oid(root))
        (root / "core.py").write_text(CORE + "\n\ndef rebate(sku):\n    return 2\n", "utf-8")
        commit(root, "second")
        live = _tree_oid(root)

        assert invoke(services, root, "map") == EXIT_OK
        receipt = _receipt(capsys.readouterr().out)

        assert indexed.generation != live
        assert f"cache: g:{indexed.generation} generation mismatch (repo g:{live})" in receipt
        # The command names the repository. An agent reads this receipt from
        # wherever it is working, which is often not the repository the
        # receipt describes, and `agentless-mcp index` with no argument would
        # then index the wrong tree or refuse.
        assert (
            f"changed files parse live; run agentless-mcp index --repo {root} for performance"
            in receipt
        )

    def test_a_mismatched_generation_still_answers_from_live_content(
        self, make_git_repo, extractor
    ):
        root = make_git_repo({"core.py": CORE})
        cache.build_index(root, extractor, tree_oid=_tree_oid(root))
        changed = CORE + "\n\ndef rebate(sku):\n    return 2\n"
        (root / "core.py").write_text(changed, encoding="utf-8")
        commit(root, "second")

        source = cache.open_source(root, extractor, tree_oid=_tree_oid(root))
        names = [symbol.name for symbol in source.symbols_for(changed, "python", "core.py")]

        assert "generation mismatch" in source.receipt
        assert "rebate" in names
        assert source.status().as_dict()["generation_matches"] is False


class TestDirtyWorktree:
    def test_an_uncommitted_edit_is_answered_from_the_live_file(
        self, make_git_repo, extractor, services, capsys
    ):
        root = make_git_repo({"core.py": CORE})
        cache.build_index(root, extractor, tree_oid=_tree_oid(root))
        changed = CORE + "\n\ndef rebate(sku):\n    return 2\n"
        (root / "core.py").write_text(changed, encoding="utf-8")

        source = cache.open_source(root, extractor, tree_oid=_tree_oid(root))
        names = [symbol.name for symbol in source.symbols_for(changed, "python", "core.py")]

        assert "fresh" in source.receipt
        assert "rebate" in names

        assert invoke(services, root, "find-symbol", "rebate") == EXIT_OK
        assert "rebate" in capsys.readouterr().out

    def test_an_unchanged_file_is_still_served_from_the_index(self, make_git_repo, extractor, spy):
        root = make_git_repo({"core.py": CORE, "billing.py": BILLING})
        cache.build_index(root, extractor, tree_oid=_tree_oid(root))
        spy.clear()

        source = cache.open_source(root, extractor, tree_oid=_tree_oid(root))
        names = [symbol.name for symbol in source.symbols_for(CORE, "python", "core.py")]

        assert spy == []
        assert "quote" in names


class TestSchemaVersion:
    def test_a_schema_bump_drops_the_database_and_rebuilds_it(self, repo, extractor, monkeypatch):
        cache.build_index(repo, extractor)
        monkeypatch.setattr(cache, "SCHEMA_VERSION", cache.SCHEMA_VERSION + 1)

        rejected = cache.open_source(repo, extractor, tree_oid=None)

        assert "schema" in rejected.receipt
        assert not cache.cache_path(repo).exists()

        rebuilt = cache.build_index(repo, extractor)
        assert cache.open_source(repo, extractor, tree_oid=None).receipt == (
            f"g:{rebuilt.generation} fresh"
        )

    def test_the_schema_holds_no_write_only_columns(self, repo, extractor):
        """A column written on every row and selected by nobody is not stored."""
        report = cache.build_index(repo, extractor)

        with closing(sqlite3.connect(report.database)) as connection:
            files = {row[1] for row in connection.execute("PRAGMA table_info(files)")}
            tags = {row[1] for row in connection.execute("PRAGMA table_info(tags)")}

        assert files == {"path", "sha256", "grammar_version"}
        assert "qualname" not in tags
        assert "is_def" not in tags

    def test_a_corrupt_database_is_discarded_by_both_readers_and_writers(self, repo, extractor):
        database = cache.build_index(repo, extractor).database
        database.write_bytes(b"not a database at all")

        rejected = cache.open_source(repo, extractor, tree_oid=None)
        assert rejected.receipt.startswith("none (cache discarded")

        rebuilt = cache.build_index(repo, extractor)
        assert rebuilt.files == 3
        assert cache.open_source(repo, extractor, tree_oid=None).receipt.endswith("fresh")

    def test_a_closed_source_releases_its_connection(self, repo, extractor):
        cache.build_index(repo, extractor)
        source = cache.open_source(repo, extractor, tree_oid=None)
        assert isinstance(source, cache.CachedSource)

        source.close()

        with pytest.raises(sqlite3.ProgrammingError):
            source.status()

    def test_a_database_built_for_another_repository_is_refused(self, repo, extractor, tmp_path):
        report = cache.build_index(repo, extractor)
        other = tmp_path / "elsewhere"
        other.mkdir()
        target = cache.cache_path(other)
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(report.database.read_bytes())

        source = cache.open_source(other, extractor, tree_oid=None)

        assert "built for" in source.receipt


class TestNonGitRepositories:
    def test_the_generation_is_a_manifest_digest(self, repo, extractor):
        report = cache.build_index(repo, extractor)

        assert report.generation.startswith(cache.NOGIT_PREFIX)

    def test_touching_a_file_moves_the_generation(self, repo, extractor):
        cache.build_index(repo, extractor)
        (repo / "core.py").write_text(CORE + "\n\ndef discount(sku):\n    return 1\n", "utf-8")

        source = cache.open_source(repo, extractor, tree_oid=None)

        assert "generation mismatch" in source.receipt


class TestSingleWriter:
    """The POSIX half of the write lock.

    ``fcntl`` is imported inside the test rather than at module scope: it does
    not exist on Windows, and a module-level import would fail collection for
    this whole file there -- taking every freshness, schema and equivalence
    test with it, none of which is about locking. The package ships a Windows
    lock implementation (``util.filelock``), so those tests have to run there.
    """

    def test_a_second_index_run_refuses_while_the_lock_is_held(self, repo, extractor):
        fcntl = pytest.importorskip("fcntl", reason="POSIX advisory locking")
        first = cache.build_index(repo, extractor)
        lock_path = first.database.parent / cache.LOCK_NAME
        handle = lock_path.open("w", encoding="utf-8")
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)

        try:
            with pytest.raises(CacheLocked, match="could not take the index lock for"):
                cache.build_index(repo, extractor)
        finally:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
            handle.close()

        source = cache.open_source(repo, extractor, tree_oid=None)
        try:
            assert source.status().files == 3
        finally:
            source.close()


class TestCommandLine:
    def test_index_summarises_what_it_did(self, repo, services, capsys):
        assert invoke(services, repo, "index") == EXIT_OK
        summary = capsys.readouterr().out.splitlines()[0]

        assert summary.startswith("indexed 3, reused 0, pruned 0, skipped 0, errors 0: 3 files, ")
        assert " imports, " in summary
        assert " refs at g:nogit:" in summary

    def test_a_second_index_run_reports_reuse(self, repo, services, capsys):
        invoke(services, repo, "index")
        capsys.readouterr()

        assert invoke(services, repo, "index") == EXIT_OK
        assert capsys.readouterr().out.startswith(
            "indexed 0, reused 3, pruned 0, skipped 0, errors 0:"
        )

    def test_force_reports_every_file_as_indexed(self, repo, services, capsys):
        invoke(services, repo, "index")
        capsys.readouterr()

        assert invoke(services, repo, "index", "--force") == EXIT_OK
        assert capsys.readouterr().out.startswith(
            "indexed 3, reused 0, pruned 0, skipped 0, errors 0:"
        )

    # The two ways ``build_index`` fails that are not the package's own error
    # class, so neither reached the handler in ``run``. Faked rather than
    # staged: a full disk is not something a hermetic test can arrange, and a
    # read-only cache directory is not refused the same way on every platform.
    STORAGE_FAILURES = (
        sqlite3.OperationalError("attempt to write a readonly database"),
        sqlite3.DatabaseError("database disk image is malformed"),
        OSError(28, "No space left on device"),
        PermissionError(13, "Permission denied"),
    )

    @pytest.mark.parametrize("error", STORAGE_FAILURES, ids=lambda e: type(e).__name__)
    def test_a_cache_that_cannot_be_written_is_a_refusal_and_not_a_traceback(
        self, repo, services, capsys, monkeypatch, error
    ):
        """``build_index`` opens and writes SQLite, so it raises what those raise.

        ``sqlite3.Error`` and ``OSError`` are not ``AgentlessError``, so the
        handler in ``run`` did not catch either one and this command ended in
        a raw traceback -- which also puts an absolute local path and the
        package's own frames on stderr, for a condition the operator can act
        on if they are simply told it.
        """

        def refuse(*arguments, **keywords):
            raise error

        monkeypatch.setattr(cache, "build_index", refuse)

        assert invoke(services, repo, "index") == EXIT_DOMAIN

        captured = capsys.readouterr()
        assert captured.out == "", "a refusal must not also print a partial report"
        assert "Traceback" not in captured.err

        # Read as one line rather than as a blob. `fail` puts a refusal on
        # exactly one line so that whatever splits stderr reads one refusal
        # and not three, and this repository is not a git tree, so a
        # degraded-repo note shares the stream with it.
        (refusal,) = [line for line in captured.err.splitlines() if "tag cache" in line]
        assert refusal.startswith("agentless-mcp: cannot build the tag cache for ")
        assert str(repo.resolve()) in refusal
        assert str(error) in refusal

    def test_a_refused_index_prints_no_json_for_a_caller_parsing_stdout(
        self, repo, services, capsys, monkeypatch
    ):
        """``--json`` fails the same way: an agent must not read half a document."""

        def refuse(*arguments, **keywords):
            message = "attempt to write a readonly database"
            raise sqlite3.OperationalError(message)

        monkeypatch.setattr(cache, "build_index", refuse)

        assert invoke(services, repo, "index", "--json") == EXIT_DOMAIN

        captured = capsys.readouterr()
        assert captured.out == ""
        (refusal,) = [line for line in captured.err.splitlines() if "tag cache" in line]
        assert refusal.startswith("agentless-mcp: cannot build the tag cache for ")

    def test_index_json_carries_the_same_numbers(self, repo, services, capsys):
        assert invoke(services, repo, "index", "--json") == EXIT_OK
        document = json.loads(capsys.readouterr().out)

        assert document["indexed"] == 3
        assert document["files"] == 3

    def test_a_read_reports_the_cache_in_its_receipt(self, repo, services, capsys):
        invoke(services, repo, "index")
        capsys.readouterr()

        assert invoke(services, repo, "map") == EXIT_OK
        assert "cache: g:nogit:" in _receipt(capsys.readouterr().out)

    def test_no_cache_is_reported_in_the_receipt(self, repo, services, capsys):
        invoke(services, repo, "index")
        capsys.readouterr()

        assert invoke(services, repo, "map", "--no-cache") == EXIT_OK
        assert "cache: bypassed (--no-cache)" in _receipt(capsys.readouterr().out)

    def test_capabilities_reports_the_cache_path_and_row_counts(self, repo, services, capsys):
        invoke(services, repo, "index")
        capsys.readouterr()

        assert invoke(services, repo, "capabilities") == EXIT_OK
        out = capsys.readouterr().out

        assert str(cache.cache_path(repo)) in out
        assert "files 3" in out


class TestEquivalence:
    @pytest.mark.parametrize("repo_name", FIXTURE_REPOS)
    def test_map_is_identical_cached_and_uncached(self, repo_name, fixtures_dir, services, capsys):
        root = fixtures_dir / repo_name
        assert invoke(services, root, "index") == EXIT_OK
        capsys.readouterr()

        assert invoke(services, root, "map") == EXIT_OK
        cached = capsys.readouterr().out
        assert invoke(services, root, "map", "--no-cache") == EXIT_OK
        uncached = capsys.readouterr().out

        assert "fresh" in _receipt(cached)
        assert body(cached) == body(uncached)

    @pytest.mark.parametrize("repo_name", FIXTURE_REPOS)
    def test_skeleton_is_identical_cached_and_uncached(
        self, repo_name, fixtures_dir, services, capsys
    ):
        root = fixtures_dir / repo_name
        target = SKELETON_FILES[repo_name]
        assert invoke(services, root, "index") == EXIT_OK
        capsys.readouterr()

        assert invoke(services, root, "skeleton", target) == EXIT_OK
        cached = capsys.readouterr().out
        assert invoke(services, root, "skeleton", target, "--no-cache") == EXIT_OK

        assert body(cached) == body(capsys.readouterr().out)

    @pytest.mark.parametrize("repo_name", FIXTURE_REPOS)
    def test_find_symbol_is_identical_cached_and_uncached(
        self, repo_name, fixtures_dir, services, capsys
    ):
        root = fixtures_dir / repo_name
        assert invoke(services, root, "index") == EXIT_OK
        capsys.readouterr()

        assert invoke(services, root, "find-symbol", "money") == EXIT_OK
        cached = capsys.readouterr().out
        assert invoke(services, root, "find-symbol", "money", "--no-cache") == EXIT_OK
        uncached = capsys.readouterr().out

        assert "money" in cached.lower()
        assert body(cached) == body(uncached)

    @pytest.mark.parametrize("repo_name", FIXTURE_REPOS)
    def test_json_output_is_identical_cached_and_uncached(
        self, repo_name, fixtures_dir, services, capsys
    ):
        root = fixtures_dir / repo_name
        assert invoke(services, root, "index") == EXIT_OK
        capsys.readouterr()

        assert invoke(services, root, "map", "--json") == EXIT_OK
        cached = json.loads(capsys.readouterr().out)
        assert invoke(services, root, "map", "--json", "--no-cache") == EXIT_OK
        uncached = json.loads(capsys.readouterr().out)

        del cached["receipt"]["cache"], uncached["receipt"]["cache"]
        assert cached == uncached


class TestRefsAndImportsRows:
    """The Phase 4 half of the cache: imports and references, same gate."""

    def test_an_index_records_rows_of_all_three_kinds(self, repo, extractor):
        report = cache.build_index(repo, extractor)

        assert report.tags > 0
        assert report.imports > 0
        assert report.refs > 0

    def test_a_fresh_index_serves_every_kind_without_parsing(self, repo, extractor, parse_spy):
        cache.build_index(repo, extractor)
        for kind in parse_spy:
            parse_spy[kind].clear()

        source = cache.open_source(repo, extractor, tree_oid=None)
        refs.scan_repo(repo, extractor, source=source)

        assert parse_spy == {"symbols": [], "imports": [], "refs": []}

    def test_touching_one_file_reparses_exactly_that_file_for_every_kind(
        self, repo, extractor, parse_spy
    ):
        cache.build_index(repo, extractor)
        (repo / "core.py").write_text(CORE + "\n\ndef discount(sku):\n    return 1\n", "utf-8")
        for kind in parse_spy:
            parse_spy[kind].clear()

        source = cache.open_source(repo, extractor, tree_oid=None)
        refs.scan_repo(repo, extractor, source=source)

        assert parse_spy == {
            "symbols": ["core.py"],
            "imports": ["core.py"],
            "refs": ["core.py"],
        }

    def test_cached_imports_match_the_parsed_ones(self, repo, extractor):
        cache.build_index(repo, extractor)
        source = cache.open_source(repo, extractor, tree_oid=None)

        cached = source.imports_for(BILLING, "python", "billing.py")
        parsed = extractor.extract_imports_from_source(BILLING, "python", "billing.py")

        assert cached == parsed
        assert cached  # the fixture does import something

    def test_cached_refs_match_the_parsed_ones(self, repo, extractor):
        cache.build_index(repo, extractor)
        source = cache.open_source(repo, extractor, tree_oid=None)

        cached = source.refs_for(CORE, "python", "core.py")
        parsed = extractor.extract_refs_from_source(CORE, "python", "core.py")

        assert cached == parsed
        assert cached

    def test_cached_rows_rebuild_the_same_collision_ordinals(self, repo, extractor):
        """The ordinal is derived, not stored, so both paths have to agree.

        A cached id that dropped the ``#2`` would address the first of two
        same-named symbols, which is the collision defect back one layer down.
        """
        source_text = "def handle():\n    return 1\n\n\ndef handle():\n    return 2\n"
        (repo / "handlers.py").write_text(source_text, encoding="utf-8")
        cache.build_index(repo, extractor)
        opened = cache.open_source(repo, extractor, tree_oid=None)

        cached = opened.symbols_for(source_text, "python", "handlers.py")
        parsed = extractor.extract_from_source(source_text, "python", "handlers.py")

        assert [symbol_stable_id(symbol) for symbol in cached] == [
            "py:handlers.py::handle",
            "py:handlers.py::handle#2",
        ]
        assert cached == parsed

    def test_cached_rows_rebuild_the_same_rationale_nodes(self, repo, extractor):
        source_text = "def handle():\n    # NOTE: ordering follows RFC 2119\n    return 1\n"
        (repo / "handlers.py").write_text(source_text, encoding="utf-8")
        cache.build_index(repo, extractor)
        opened = cache.open_source(repo, extractor, tree_oid=None)

        cached = opened.symbols_for(source_text, "python", "handlers.py")
        parsed = extractor.extract_from_source(source_text, "python", "handlers.py")

        assert cached == parsed
        assert cached[0].rationales[0].citations == ("RFC 2119",)

    def test_an_edited_file_falls_back_to_parsing_for_every_kind(self, repo, extractor):
        cache.build_index(repo, extractor)
        source = cache.open_source(repo, extractor, tree_oid=None)
        changed = CORE + "\n\ndef rebate(sku):\n    return 2\n"

        assert "rebate" in {
            symbol.name for symbol in source.symbols_for(changed, "python", "core.py")
        }
        assert "rebate" in {ref.name for ref in source.refs_for(changed, "python", "core.py")}
        assert source.imports_for(changed, "python", "core.py") == (
            extractor.extract_imports_from_source(changed, "python", "core.py")
        )

    def test_a_pruned_file_leaves_no_rows_behind(self, repo, extractor):
        first = cache.build_index(repo, extractor)
        (repo / "billing.py").unlink()

        second = cache.build_index(repo, extractor)

        assert second.pruned == 1
        queries = (
            "SELECT COUNT(*) FROM tags WHERE path = ?",
            "SELECT COUNT(*) FROM imports WHERE path = ?",
            "SELECT COUNT(*) FROM refs WHERE path = ?",
            "SELECT COUNT(*) FROM files WHERE path = ?",
        )
        with closing(sqlite3.connect(second.database)) as connection:
            for query in queries:
                assert connection.execute(query, ("billing.py",)).fetchone()[0] == 0
        assert first.files == 3


class TestRefsEquivalence:
    @pytest.mark.parametrize("repo_name", FIXTURE_REPOS)
    def test_refs_are_identical_cached_and_uncached(
        self, repo_name, fixtures_dir, services, capsys
    ):
        root = fixtures_dir / repo_name
        assert invoke(services, root, "index") == EXIT_OK
        capsys.readouterr()

        assert invoke(services, root, "refs", "money", "--shared-callers") == EXIT_OK
        cached = capsys.readouterr().out
        assert invoke(services, root, "refs", "money", "--shared-callers", "--no-cache") == EXIT_OK

        assert body(cached) == body(capsys.readouterr().out)


def _receipt(output: str) -> str:
    """Return the receipt line of one rendered answer."""
    return next(line for line in output.splitlines() if line.startswith("# repo:"))


def _tree_oid(root) -> str:
    """Return the live tree OID of ``root``, as the receipt reports it."""
    oid = gitinfo.tree_oid(root)
    assert oid is not None
    return oid


@pytest.fixture
def auto_index_isolated(monkeypatch):
    """A clean per-test auto-index state, with the suite-wide opt-out lifted."""
    monkeypatch.delenv(cache.ENV_NO_AUTO_INDEX, raising=False)
    monkeypatch.setattr(cache, "_AUTO_INDEX_RUNS", {})


class TestAutoIndex:
    """Issue #21: the background refresh of a stale tag cache."""

    def test_first_use_builds_the_absent_index(self, repo, extractor, auto_index_isolated):
        thread = cache.start_auto_index(repo, extractor)
        assert thread is not None
        thread.join(timeout=30)
        assert not thread.is_alive()

        source = cache.open_source(repo, extractor, tree_oid=None)
        assert source.receipt.endswith("fresh")

    def test_a_current_index_starts_nothing(self, repo, extractor, auto_index_isolated):
        cache.build_index(repo, extractor)
        assert cache.start_auto_index(repo, extractor) is None

    def test_the_environment_opt_out_keeps_the_refresh_off(
        self, repo, extractor, monkeypatch, auto_index_isolated
    ):
        monkeypatch.setenv(cache.ENV_NO_AUTO_INDEX, "1")
        monkeypatch.setattr(cache, "build_index", self._must_not_index)
        assert cache.start_auto_index(repo, extractor) is None
        assert cache.auto_index_in_progress(cache.cache_path(repo)) is False

    def test_a_running_refresh_is_not_duplicated(
        self, repo, extractor, monkeypatch, auto_index_isolated
    ):
        release = threading.Event()
        real = cache.build_index

        def slow(root, extractor, *, tree_oid=None, head_sha=None, force=False):
            release.wait(timeout=10)
            return real(root, extractor, tree_oid=tree_oid, head_sha=head_sha, force=force)

        monkeypatch.setattr(cache, "build_index", slow)
        thread = cache.start_auto_index(repo, extractor)
        assert thread is not None
        # The start returned while the refresh is still running: the caller is
        # served live first and the cache lands later.
        assert thread.is_alive()
        # A second first-use of the same repository joins the same refresh.
        assert cache.start_auto_index(repo, extractor) is thread
        release.set()
        thread.join(timeout=30)
        assert not thread.is_alive()

    def test_one_attempt_per_generation_even_after_failure(
        self, repo, extractor, monkeypatch, auto_index_isolated, caplog
    ):
        def exploding(root, extractor, *, tree_oid=None, head_sha=None, force=False):
            message = "boom"
            raise OperationFailed(message)

        monkeypatch.setattr(cache, "build_index", exploding)
        with caplog.at_level(logging.WARNING, logger="agentless_mcp.core.cache"):
            thread = cache.start_auto_index(repo, extractor)
            assert thread is not None
            thread.join(timeout=10)
        assert "background index refresh" in caplog.text
        assert "failed: boom" in caplog.text

        # The generation did not move, so the failed attempt is not retried:
        # a broken build must not become a per-call retry storm.
        monkeypatch.setattr(cache, "build_index", self._must_not_index)
        assert cache.start_auto_index(repo, extractor) is None

    def test_a_generation_that_cannot_be_read_is_attempted_once(
        self, repo, extractor, monkeypatch, auto_index_isolated, caplog
    ):
        """The failure that stops before a thread is registered like any other.

        The walk this path pays for is a stat per file in the repository, and
        it ran again, and warned again, on every MCP call.
        """
        walks: list[Path] = []

        def refuse(root, tree_oid):
            walks.append(root)
            message = "the tree cannot be walked"
            raise OSError(message)

        monkeypatch.setattr(cache, "repo_generation", refuse)
        with caplog.at_level(logging.WARNING, logger="agentless_mcp.core.cache"):
            assert cache.start_auto_index(repo, extractor) is None
            assert cache.start_auto_index(repo, extractor) is None

        assert len(walks) == 1
        assert caplog.text.count("background index refresh") == 1
        assert cache.auto_index_in_progress(cache.cache_path(repo)) is False

    def test_a_new_generation_rearms_the_trigger(self, repo, extractor, auto_index_isolated):
        first = cache.start_auto_index(repo, extractor)
        assert first is not None
        first.join(timeout=30)

        (repo / "core.py").write_text(CORE + "\nEXTRA = 1\n", encoding="utf-8")
        second = cache.start_auto_index(repo, extractor)
        assert second is not None
        assert second is not first
        second.join(timeout=30)
        source = cache.open_source(repo, extractor, tree_oid=None)
        assert source.receipt.endswith("fresh")

    def test_a_held_lock_is_a_silent_skip(self, repo, extractor, auto_index_isolated, caplog):
        database = cache.cache_path(repo)
        database.parent.mkdir(parents=True, exist_ok=True)
        with (
            caplog.at_level(logging.INFO, logger="agentless_mcp.core.cache"),
            cache._write_lock(database.parent, repo),
        ):
            cache._auto_index(repo, extractor, None, None)
        # "index run" rather than "process": the lock is per open file
        # description, so the second thread of this process is refused
        # identically and reads the same line.
        assert "another index run holds the index lock" in caplog.text
        assert "failed" not in caplog.text

    def test_the_receipt_names_the_refresh_while_it_runs(
        self, repo, extractor, monkeypatch, auto_index_isolated
    ):
        cache.build_index(repo, extractor)
        (repo / "core.py").write_text(CORE + "\nEXTRA = 1\n", encoding="utf-8")

        release = threading.Event()
        real = cache.build_index

        def slow(root, extractor, *, tree_oid=None, head_sha=None, force=False):
            release.wait(timeout=10)
            return real(root, extractor, tree_oid=tree_oid, head_sha=head_sha, force=force)

        monkeypatch.setattr(cache, "build_index", slow)
        thread = cache.start_auto_index(repo, extractor)
        assert thread is not None
        try:
            stale = cache.open_source(repo, extractor, tree_oid=None)
            assert "a background refresh is in progress" in stale.receipt
            assert "run agentless-mcp index" not in stale.receipt
        finally:
            release.set()
            thread.join(timeout=30)

        # Once the refresh has landed, the remediation is gone with the
        # mismatch itself.
        fresh = cache.open_source(repo, extractor, tree_oid=None)
        assert fresh.receipt.endswith("fresh")

    @staticmethod
    def _must_not_index(root, extractor, *, tree_oid=None, head_sha=None, force=False):
        pytest.fail("the background refresh must not run here")


class TestAutoIndexStartRace:
    """One index per generation, including across the registration window.

    A thread that is registered but not yet started reads as
    ``is_alive() == False``. A second caller that had already found the
    registry empty therefore saw no live run, fell past the guard, and started
    a duplicate index of the same generation. The window is narrow -- it is the
    gap between the dict assignment and ``thread.start()`` -- so this drives the
    interleaving directly rather than hoping to hit it.
    """

    def test_a_caller_arriving_in_the_registration_window_starts_nothing_new(
        self, repo, extractor, monkeypatch, auto_index_isolated
    ):
        builds = []
        real_build = cache.build_index

        def counting(root, extractor, *, tree_oid=None, head_sha=None, force=False):
            builds.append(threading.current_thread().name)
            return real_build(root, extractor, tree_oid=tree_oid, head_sha=head_sha, force=force)

        monkeypatch.setattr(cache, "build_index", counting)

        second_read_the_empty_registry = threading.Event()
        first_registered_its_run = threading.Event()
        real_index_current = cache._index_current

        def gated(database, repo_root, generation):
            # Park the second caller after it has seen an empty registry and
            # before it re-checks, so it re-checks inside the window.
            if threading.current_thread().name == "second":
                second_read_the_empty_registry.set()
                first_registered_its_run.wait(timeout=10)
            return real_index_current(database, repo_root, generation)

        monkeypatch.setattr(cache, "_index_current", gated)

        real_thread = threading.Thread

        class RegisteredButNotStarted(real_thread):
            def start(self):
                first_registered_its_run.set()
                # Hold the registered-but-not-alive window open.
                time.sleep(0.5)
                super().start()

        monkeypatch.setattr(cache.threading, "Thread", RegisteredButNotStarted)

        handed: dict[str, object] = {}

        def caller(tag):
            handed[tag] = cache.start_auto_index(repo, extractor)

        second = real_thread(target=caller, args=("second",), name="second")
        second.start()
        assert second_read_the_empty_registry.wait(timeout=10)
        first = real_thread(target=caller, args=("first",), name="first")
        first.start()

        first.join(timeout=30)
        second.join(timeout=30)
        for thread in handed.values():
            if thread is not None:
                thread.join(timeout=30)

        assert len(builds) == 1, f"the generation was indexed {len(builds)} times: {builds}"


class TestTheReadViewIsPinned:
    """A row read must see the database the freshness gate approved.

    `_fresh_digest` consults the entries snapshot taken when the source
    opened; the row reads that follow were separate autocommit statements,
    each seeing the database at its own instant. The background index thread
    that this very request starts writes between the two, and `_symbol_rows`
    then returned an empty list -- which the caller cannot tell apart from a
    file that genuinely defines nothing.
    """

    def test_rows_deleted_underneath_a_source_do_not_empty_its_answer(
        self, repo, extractor, isolated_cache_home
    ):
        cache.build_index(repo, extractor)
        source = cache.open_source(repo, extractor, tree_oid=None)
        text = (repo / "core.py").read_text(encoding="utf-8")
        assert [symbol.name for symbol in source.symbols_for(text, "python", "core.py")]

        # A second connection, as the background index thread would be.
        writer = sqlite3.connect(cache.cache_path(repo.resolve()), isolation_level=None)
        try:
            writer.execute("DELETE FROM tags")
        finally:
            writer.close()

        after = source.symbols_for(text, "python", "core.py")
        assert [symbol.name for symbol in after] == ["RATE", "quote", "PriceBook", "cost_of"]

    def test_the_snapshot_is_released_when_the_source_closes(
        self, repo, extractor, isolated_cache_home
    ):
        # The open transaction is what holds the WAL from checkpointing, so a
        # source that never released it would grow the file for the life of
        # the process. Proved by writing from another connection after close.
        cache.build_index(repo, extractor)
        source = cache.open_source(repo, extractor, tree_oid=None)
        source.close()

        writer = sqlite3.connect(cache.cache_path(repo.resolve()), isolation_level=None)
        try:
            writer.execute("PRAGMA journal_mode=WAL")
            assert writer.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()[0] == 0
        finally:
            writer.close()


class TestUnreadableRowsDegrade:
    """The module promises that a read command never fails because of a cache."""

    def _corrupt(self, repo, column, value):
        connection = sqlite3.connect(cache.cache_path(repo.resolve()), isolation_level=None)
        try:
            connection.execute(f"UPDATE tags SET {column} = ?", (value,))  # noqa: S608
        finally:
            connection.close()

    def test_a_kind_this_build_cannot_spell_is_parsed_instead(
        self, repo, extractor, isolated_cache_home, caplog
    ):
        cache.build_index(repo, extractor)
        self._corrupt(repo, "kind", "a-kind-from-another-version")
        source = cache.open_source(repo, extractor, tree_oid=None)
        text = (repo / "core.py").read_text(encoding="utf-8")

        with caplog.at_level(logging.WARNING, logger=cache.logger.name):
            names = [symbol.name for symbol in source.symbols_for(text, "python", "core.py")]

        assert names == ["RATE", "quote", "PriceBook", "cost_of"]
        assert "are unreadable" in caplog.text
        assert "core.py" in caplog.text

    def test_malformed_rationale_json_is_parsed_instead(self, repo, extractor, isolated_cache_home):
        cache.build_index(repo, extractor)
        self._corrupt(repo, "rationales", "{not json")
        source = cache.open_source(repo, extractor, tree_oid=None)
        text = (repo / "core.py").read_text(encoding="utf-8")

        assert [symbol.name for symbol in source.symbols_for(text, "python", "core.py")] == [
            "RATE",
            "quote",
            "PriceBook",
            "cost_of",
        ]


class TestDiscardingIsNotUnlocked:
    def test_a_database_being_written_is_not_deleted_by_a_reader(
        self, repo, extractor, isolated_cache_home, monkeypatch, caplog
    ):
        """Two installs each opening the other's database must not race to delete it.

        The write path's delete is sound because it holds the index lock. The
        read path's was the same delete with no lock, so a build that judges
        the database unusable could remove the one another build had just
        finished writing -- forever, and silently, because discarding is a
        degradation rather than an error.
        """
        cache.build_index(repo, extractor)
        database = cache.cache_path(repo.resolve())

        def unusable(connection, root):
            message = "simulated: written by another version"
            raise sqlite3.DatabaseError(message)

        monkeypatch.setattr(cache, "_read_index", unusable)

        with (
            filelock.exclusive(database.parent / cache.LOCK_NAME, flavour="posix"),
            caplog.at_level(logging.INFO, logger=cache.logger.name),
        ):
            source = cache.open_source(repo, extractor, tree_oid=None)

        assert database.exists(), "a reader deleted a database an indexer was holding"
        assert "left in place" in caplog.text
        assert "cache discarded" in source.receipt or source.receipt

    def test_an_unusable_database_is_still_discarded_when_nobody_holds_the_lock(
        self, repo, extractor, isolated_cache_home, monkeypatch
    ):
        cache.build_index(repo, extractor)
        database = cache.cache_path(repo.resolve())

        def unusable(connection, root):
            message = "simulated: written by another version"
            raise sqlite3.DatabaseError(message)

        monkeypatch.setattr(cache, "_read_index", unusable)
        cache.open_source(repo, extractor, tree_oid=None)

        assert not database.exists()


class TestATransientReadErrorKeepsTheDatabase:
    """An unreadable database is not the same fact as an absent one.

    ``OperationalError`` covers a disk I/O error, a file that will not open
    and a lock wait that timed out as well as a missing table. Reading any of
    them as "no index" unlinked a healthy database, because the read path
    answers an absent index by deleting the file.
    """

    def test_an_operational_error_degrades_without_deleting(self, repo, extractor, monkeypatch):
        cache.build_index(repo, extractor)
        database = cache.cache_path(repo)

        def refuse(connection, repo_root):
            message = "disk I/O error"
            raise sqlite3.OperationalError(message)

        monkeypatch.setattr(cache, "_read_index", refuse)
        source = cache.open_source(repo, extractor, tree_oid=None)

        assert "cache unreadable" in source.receipt
        assert database.exists()

    def test_a_database_with_no_meta_table_still_reads_as_absent(self, tmp_path):
        database = tmp_path / "tags.db"
        with closing(sqlite3.connect(database)) as connection:
            connection.execute("CREATE TABLE unrelated (x INTEGER)")

            assert cache._read_meta(connection) is None


# ---------------------------------------------------------------------------
# The schemas this branch's four version bumps left behind
# ---------------------------------------------------------------------------
#
# Verbatim from the commits that wrote them, because the migration has to be
# tested against what a user's cache actually holds and not against a
# paraphrase of it. They are historical constants: nothing can change them
# now. What separates them is the columns this build no longer supplies --
# every one of them declared NOT NULL, which is why reading an old database
# rather than dropping it fails on the first file of every index run.

_META_TABLE = """
CREATE TABLE meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version INTEGER NOT NULL,
    repo_root TEXT NOT NULL,
    generation_tree_oid TEXT NOT NULL,
    head_sha TEXT,
    created_at TEXT NOT NULL
);
"""

# v7 through v9: `files.size`, `files.lang`, `tags.is_def` and `tags.qualname`
# are all still present and all still NOT NULL.
_FILES_AND_TAGS_BEFORE_V10 = """
CREATE TABLE files (
    path TEXT PRIMARY KEY,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL,
    lang TEXT NOT NULL,
    grammar_version TEXT NOT NULL
);
CREATE TABLE tags (
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    is_def INTEGER NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER,
    signature TEXT NOT NULL,
    parent TEXT NOT NULL,
    qualname TEXT NOT NULL,
    docstring TEXT NOT NULL,
    decorators TEXT NOT NULL,
    bases TEXT NOT NULL,
    language TEXT NOT NULL,
    is_public INTEGER NOT NULL,
    is_async INTEGER NOT NULL,
    rationales TEXT NOT NULL,
    ordinal INTEGER NOT NULL
);
"""

_FILES_AND_TAGS_V10 = """
CREATE TABLE files (
    path TEXT PRIMARY KEY,
    sha256 TEXT NOT NULL,
    grammar_version TEXT NOT NULL
);
CREATE TABLE tags (
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER,
    signature TEXT NOT NULL,
    parent TEXT NOT NULL,
    docstring TEXT NOT NULL,
    decorators TEXT NOT NULL,
    bases TEXT NOT NULL,
    language TEXT NOT NULL,
    is_public INTEGER NOT NULL,
    is_async INTEGER NOT NULL,
    rationales TEXT NOT NULL,
    ordinal INTEGER NOT NULL
);
"""

# The imports table is what v8 and v9 changed, and `resolved_path` survives
# all four of them -- it is v11 that drops it.
_BINDS_ALL = "    binds_all INTEGER NOT NULL,"
_ALIAS = "    alias TEXT NOT NULL,"
_LOCAL_NAMES = "    local_names TEXT NOT NULL,"

_IMPORTS_ADDITIONS = {
    7: (),
    8: (_BINDS_ALL,),
    9: (_BINDS_ALL, _ALIAS, _LOCAL_NAMES),
    10: (_BINDS_ALL, _ALIAS, _LOCAL_NAMES),
}

_REFS_AND_INDEXES = """
CREATE TABLE refs (
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    name TEXT NOT NULL,
    line INTEGER NOT NULL,
    role TEXT NOT NULL,
    qualifier TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (path, sha256, ordinal)
) WITHOUT ROWID;
CREATE INDEX tags_path_sha256 ON tags (path, sha256);
CREATE INDEX imports_path_sha256 ON imports (path, sha256);
"""


def _historic_schema(version):
    """The complete schema the named version wrote."""
    files_and_tags = _FILES_AND_TAGS_V10 if version == 10 else _FILES_AND_TAGS_BEFORE_V10
    lines = [
        "CREATE TABLE imports (",
        "    path TEXT NOT NULL,",
        "    sha256 TEXT NOT NULL,",
        "    module TEXT NOT NULL,",
        "    names TEXT NOT NULL,",
        "    is_relative INTEGER NOT NULL,",
        "    relative_level INTEGER NOT NULL,",
        "    line INTEGER NOT NULL,",
        "    resolved_path TEXT NOT NULL,",
        *_IMPORTS_ADDITIONS[version],
        "    ordinal INTEGER NOT NULL",
        ");",
    ]
    return _META_TABLE + files_and_tags + "\n".join(lines) + "\n" + _REFS_AND_INDEXES


OLDER_SCHEMA_VERSIONS = (7, 8, 9, 10)


def _write_historic_database(repo, version):
    """Put a database of the named older schema where this repository's cache goes."""
    database = cache.cache_path(repo.resolve())
    database.parent.mkdir(parents=True, exist_ok=True)
    with closing(sqlite3.connect(database, isolation_level=None)) as connection:
        connection.executescript(_historic_schema(version))
        connection.execute(
            "INSERT INTO meta (id, schema_version, repo_root, generation_tree_oid, head_sha, "
            "created_at) VALUES (1, ?, ?, 'nogit:0000000000000000', NULL, '2026-01-01T00:00:00Z')",
            (version, str(repo.resolve())),
        )
    return database


def _columns(connection, table):
    """The column names one table declares."""
    return {row[1] for row in connection.execute(f"PRAGMA table_info({table})")}


class TestMigrationFromEveryOlderSchema:
    """A database at any version this branch passed through is rebuilt, not read.

    The read path deletes a database whose schema version it does not
    recognise, so the only test there was never reached ``_ensure_schema``
    with an old database at all. This is the upgrade a user actually
    performs: ``agentless-mcp index`` against the cache the previous release
    left behind. The merge-base is 7 and the branch bumps four times, so all
    four have to survive, not only the last one.
    """

    @pytest.mark.parametrize("version", OLDER_SCHEMA_VERSIONS)
    def test_an_older_database_is_rebuilt_with_this_schema(self, repo, extractor, version):
        database = _write_historic_database(repo, version)

        report = cache.build_index(repo, extractor)

        assert report.files == 3
        with closing(sqlite3.connect(database)) as connection:
            assert _columns(connection, "files") == {"path", "sha256", "grammar_version"}
            assert {"qualname", "is_def"}.isdisjoint(_columns(connection, "tags"))
            imports = _columns(connection, "imports")
            assert "resolved_path" not in imports
            assert {"binds_all", "alias", "local_names"} <= imports
            assert _read_schema_version(connection) == cache.SCHEMA_VERSION

    @pytest.mark.parametrize("version", OLDER_SCHEMA_VERSIONS)
    def test_the_rebuilt_database_reads_back_as_fresh(self, repo, extractor, version):
        _write_historic_database(repo, version)

        report = cache.build_index(repo, extractor)
        source = cache.open_source(repo, extractor, tree_oid=None)

        assert source.receipt == f"g:{report.generation} fresh"

    def test_a_database_at_this_version_is_kept(self, repo, extractor):
        """The drop is for a mismatch only: a current database keeps its rows."""
        cache.build_index(repo, extractor)

        second = cache.build_index(repo, extractor)

        assert second.reused == 3
        assert second.indexed == 0


# Spelled 11, not ``cache.SCHEMA_VERSION - 1``. The literal is the point: it
# names the one branch-internal version this bump exists to invalidate, and it
# is what makes reverting the constant to 11 fail these tests instead of
# quietly re-aiming them at whatever the previous number happens to be.
INTERMEDIATE_SCHEMA_VERSION = 11

# A name no extractor would ever produce from the fixture source, so finding it
# after an index run means exactly one thing: the row was carried over.
STALE_TAG_NAME = "symbol_the_pre_fix_extractor_wrote"


def _seed_reusable_database(repo, version):
    """A cache of ``core.py`` at ``version`` holding a row this build would never write.

    Reusable on purpose. The ``files`` row carries the file's real digest and
    the installed pack version, which is exactly the pair ``_plan_index``
    tests before it decides to skip re-extraction, so a run that does not
    discard this database will keep its rows rather than replace them.

    Built from ``cache._SCHEMA`` rather than from a transcribed older one,
    because that is the whole difficulty: v11 and v12 declare the same
    columns. Nothing about the shape can fail, so nothing about the shape can
    catch a missed discard either.
    """
    database = cache.cache_path(repo.resolve())
    database.parent.mkdir(parents=True, exist_ok=True)
    digest = cache.content_digest((repo / "core.py").read_text(encoding="utf-8"))
    with closing(sqlite3.connect(database, isolation_level=None)) as connection:
        connection.executescript(cache._SCHEMA)
        connection.execute(
            "INSERT INTO meta (id, schema_version, repo_root, generation_tree_oid, head_sha, "
            "created_at) VALUES (1, ?, ?, 'nogit:0000000000000000', NULL, '2026-01-01T00:00:00Z')",
            (version, str(repo.resolve())),
        )
        connection.execute(
            "INSERT INTO files (path, sha256, grammar_version) VALUES ('core.py', ?, ?)",
            (digest, grammars.pack_version()),
        )
        connection.execute(
            "INSERT INTO tags (path, sha256, name, kind, start_line, end_line, signature, "
            "parent, docstring, decorators, bases, language, is_public, is_async, rationales, "
            "ordinal) VALUES ('core.py', ?, ?, 'function', 1, 1, 'def stale()', '', '', "
            "'[]', '[]', 'python', 1, 0, '[]', 0)",
            (digest, STALE_TAG_NAME),
        )
    return database


class TestAnIntermediateCacheIsDiscardedForItsContents:
    """The four older schemas are self-enforcing. This one is not.

    A v7 through v10 database carries columns this build no longer writes, so
    skipping the drop fails loudly on the first INSERT of every run -- the
    tests above would catch it whatever they asserted. v11 declares exactly
    the columns v12 declares. Nothing fails; the rows are simply the ones the
    pre-fix extractor wrote, with C and C++ methods missing, import bindings
    and ``is_public`` wrong, and the reuse gate happily skipping past them.

    No released build ever wrote v11 -- ``main`` is still at v7 -- so this
    protects nobody's installed cache and every developer who ran this branch
    mid-flight. It also only works because the staleness check is ``!=``:
    these tests pass at 12 and fail at 11, which is what makes the bump
    load-bearing rather than decorative.
    """

    def test_the_stale_rows_are_re_extracted_rather_than_reused(self, repo, extractor):
        database = _seed_reusable_database(repo, INTERMEDIATE_SCHEMA_VERSION)

        report = cache.build_index(repo, extractor)

        assert report.reused == 0, "rows from another version were carried over"
        with closing(sqlite3.connect(database)) as connection:
            names = {row[0] for row in connection.execute("SELECT name FROM tags")}
            assert _read_schema_version(connection) == cache.SCHEMA_VERSION

        assert STALE_TAG_NAME not in names, "a pre-fix row survived the version bump"
        assert {"quote", "PriceBook", "cost_of"} <= names

    def test_a_reader_is_never_served_the_stale_rows(self, repo, extractor):
        """The harm, stated directly: what an answer is built from.

        The read path has its own copy of the same check, so it is its own
        test. Left un-discarded, this database answers a query about an
        untouched file from rows nobody re-derived.
        """
        _seed_reusable_database(repo, INTERMEDIATE_SCHEMA_VERSION)
        text = (repo / "core.py").read_text(encoding="utf-8")

        source = cache.open_source(repo, extractor, tree_oid=None)
        served = {symbol.name for symbol in source.symbols_for(text, "python", "core.py")}

        assert STALE_TAG_NAME not in served, "a reader was served a pre-fix row"
        assert {"quote", "PriceBook", "cost_of"} <= served

    def test_a_database_from_a_newer_build_is_discarded_too(self, repo, extractor):
        """The check is "not this version", not "older than this version".

        The same developer who holds a stale v11 cache is the one who switches
        back and forth, so the newer-than direction is not hypothetical: a
        build that reads a database written by a later one is reading rows
        whose meaning it does not know, which is the same defect pointed the
        other way. Written as ``<`` the drop would fire in only one direction
        and this case would be served rather than rebuilt.
        """
        database = _seed_reusable_database(repo, cache.SCHEMA_VERSION + 1)

        report = cache.build_index(repo, extractor)

        assert report.reused == 0, "rows written by a newer build were carried over"
        with closing(sqlite3.connect(database)) as connection:
            names = {row[0] for row in connection.execute("SELECT name FROM tags")}
            assert _read_schema_version(connection) == cache.SCHEMA_VERSION

        assert STALE_TAG_NAME not in names


class TestTheMigrationIsOneTransaction:
    """An interrupted migration must leave a whole database, not half of each.

    The drop is five statements and the create is five more. Outside a
    transaction each commits on its own, so a process killed between them
    leaves tables from both schemas and a ``meta`` row that may or may not
    have survived to say which. Recovery then rests on the order the drops
    happen to be written in. Inside one, there is no such state to land in.
    """

    def test_a_migration_that_does_not_commit_leaves_the_old_schema(self, tmp_path):
        database = tmp_path / "tags.db"
        with closing(sqlite3.connect(database, isolation_level=None)) as connection:
            connection.executescript(_historic_schema(7))
            connection.executescript(cache._MIGRATE_SCHEMA.replace(cache._COMMIT, "ROLLBACK;\n"))

            assert "size" in _columns(connection, "files")
            assert "qualname" in _columns(connection, "tags")
            assert "resolved_path" in _columns(connection, "imports")

    def test_a_create_that_does_not_commit_leaves_no_tables(self, tmp_path):
        database = tmp_path / "tags.db"
        with closing(sqlite3.connect(database, isolation_level=None)) as connection:
            connection.executescript(cache._CREATE_SCHEMA.replace(cache._COMMIT, "ROLLBACK;\n"))

            assert not cache._has_table(connection, "meta")
            assert not cache._has_table(connection, "files")


class TestAnUnreadableMetaRowIsNotAnAbsentOne:
    """``_ensure_schema`` swallowed the error ``_read_meta`` was changed to raise.

    Reported as "no meta row", it took the not-stale branch, left every old
    table in place behind ``CREATE TABLE IF NOT EXISTS``, and made
    ``_open_for_write``'s own discard-and-rebuild handler unreachable for the
    case its docstring names.
    """

    def test_a_meta_row_that_cannot_be_read_discards_and_rebuilds(
        self, repo, extractor, monkeypatch
    ):
        cache.build_index(repo, extractor)
        attempts = []
        real = cache._read_meta

        def refuse_once(connection):
            attempts.append(1)
            if len(attempts) == 1:
                message = "database disk image is malformed"
                raise sqlite3.DatabaseError(message)
            return real(connection)

        monkeypatch.setattr(cache, "_read_meta", refuse_once)

        report = cache.build_index(repo, extractor)

        assert len(attempts) == 2, "the second open must re-read the meta row it just rebuilt"
        assert report.files == 3
        assert report.reused == 0, "rows from a database judged unusable were read back as good"


class TestTheFileScanObeysTheSameRuleAsTheMetaRead:
    """``_load_entries`` answered a failed scan with an empty mapping.

    That is the same swallow ``_read_meta`` was fixed for, one table over,
    and it degrades worse: an empty mapping is not "no index", it is "an
    index of a repository with no files", so every digest missed, every file
    was re-parsed, and the receipt still said ``fresh``.
    """

    def test_an_unreadable_file_table_is_not_an_empty_one(self, tmp_path):
        database = tmp_path / "tags.db"
        with closing(sqlite3.connect(database)) as connection:
            connection.execute("CREATE TABLE unrelated (x INTEGER)")

            with pytest.raises(sqlite3.OperationalError):
                cache._load_entries(connection)

    def test_a_missing_file_table_degrades_without_deleting(self, repo, extractor):
        cache.build_index(repo, extractor)
        database = cache.cache_path(repo)
        with closing(sqlite3.connect(database)) as connection:
            connection.execute("DROP TABLE files")

        source = cache.open_source(repo, extractor, tree_oid=None)

        assert "cache unreadable" in source.receipt
        assert "fresh" not in source.receipt
        assert database.exists()


class TestAnUnreadableIndexSaysWhyItIsRefreshed:
    """An unreadable index starts a full run, so it has to say that is why.

    Answered silently, a failing disk and an ordinary stale generation stamp
    produced the same refresh and left nothing behind to tell them apart.
    """

    def test_a_failed_open_is_logged_with_the_database_and_the_reason(
        self, repo, extractor, monkeypatch, caplog
    ):
        cache.build_index(repo, extractor)
        database = cache.cache_path(repo.resolve())

        def refuse(path):
            message = "unable to open database file"
            raise sqlite3.OperationalError(message)

        monkeypatch.setattr(cache, "_connect", refuse)
        with caplog.at_level(logging.WARNING, logger=cache.logger.name):
            assert cache._index_current(database, repo.resolve(), "nogit:whatever") is False

        assert str(database) in caplog.text
        assert "unable to open database file" in caplog.text

    def test_a_failed_meta_read_is_logged_with_the_database_and_the_reason(
        self, repo, extractor, monkeypatch, caplog
    ):
        cache.build_index(repo, extractor)
        database = cache.cache_path(repo.resolve())

        def refuse(connection):
            message = "disk I/O error"
            raise sqlite3.OperationalError(message)

        monkeypatch.setattr(cache, "_read_meta", refuse)
        with caplog.at_level(logging.WARNING, logger=cache.logger.name):
            assert cache._index_current(database, repo.resolve(), "nogit:whatever") is False

        assert str(database) in caplog.text
        assert "disk I/O error" in caplog.text


def test_the_two_copies_of_the_degraded_error_list_agree():
    """One rule in two files, and they had already drifted apart once.

    ``core.patchlint`` cannot import ``core.cache`` -- it sits above it in the
    module graph -- so the list of what a parse is allowed to fail with is
    written out twice on purpose. At the merge-base the two had four members
    and five. Nothing but this test keeps them equal.
    """
    assert cache.EXTRACTION_FAILURES == patchlint.DEGRADED_ERRORS


def _read_schema_version(connection):
    """The schema version the meta row records."""
    return connection.execute("SELECT schema_version FROM meta WHERE id = 1").fetchone()[0]
