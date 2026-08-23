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
import subprocess
import threading
import time
from contextlib import closing
from pathlib import Path

import pytest

from agentless_mcp.adapters.cli.formatting import EXIT_OK
from agentless_mcp.adapters.cli.main import CliServices, run
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.lint_service import LintService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.validate_service import ValidateService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import cache, gitinfo, refs
from agentless_mcp.core.symbols import symbol_stable_id
from agentless_mcp.util import filelock, fslimits
from agentless_mcp.util.errors import AtlasError, CacheLocked

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
        oversize = repo / "huge.py"
        oversize.write_text(CORE, encoding="utf-8")
        monkeypatch.setattr(cache, "read_bounded", _refuse_one("huge.py"))

        report = cache.build_index(repo, extractor)

        assert report.errors == 1
        assert report.failures[0].path == "huge.py"
        assert "too big" in report.failures[0].reason

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
            return fslimits.BoundedRead(path=path, text=None, skipped="skipped: too big")
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
        assert "changed files parse live; reindex for performance" in receipt

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
            raise AtlasError(message)

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
        assert "another process holds the index lock" in caplog.text
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
            assert "reindex for performance" not in stale.receipt
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


class TestRelativeCacheHomeIsIgnored:
    """A relative XDG_CACHE_HOME resolves against the working directory."""

    def test_a_relative_value_falls_back_to_the_default(self, monkeypatch, tmp_path, caplog):
        # Reproduced during the audit: `cd victim && XDG_CACHE_HOME=relcache
        # ... validate --repo victim` created victim/relcache/agentless-mcp/
        # worktrees inside the repository being analysed.
        monkeypatch.setattr(cache, "_RELATIVE_CACHE_HOMES_SEEN", set())
        monkeypatch.setenv(cache.ENV_CACHE_HOME, "relcache")
        monkeypatch.setattr(Path, "home", lambda: tmp_path)

        with caplog.at_level(logging.WARNING, logger=cache.logger.name):
            root = cache.cache_root()

        assert root == tmp_path / ".cache" / cache.APPLICATION_DIR
        assert "is relative and was ignored" in caplog.text

    def test_the_warning_is_not_repeated_for_the_same_value(self, monkeypatch, tmp_path, caplog):
        monkeypatch.setattr(cache, "_RELATIVE_CACHE_HOMES_SEEN", set())
        monkeypatch.setenv(cache.ENV_CACHE_HOME, "relcache")
        monkeypatch.setattr(Path, "home", lambda: tmp_path)

        with caplog.at_level(logging.WARNING, logger=cache.logger.name):
            for _ in range(5):
                cache.cache_root()

        assert caplog.text.count("is relative and was ignored") == 1

    def test_an_absolute_value_is_honoured(self, monkeypatch, tmp_path):
        monkeypatch.setenv(cache.ENV_CACHE_HOME, str(tmp_path / "elsewhere"))
        assert cache.cache_root() == tmp_path / "elsewhere" / cache.APPLICATION_DIR
