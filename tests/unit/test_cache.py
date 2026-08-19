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

import fcntl
import json
import sqlite3
import subprocess

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
from agentless_mcp.util import fslimits
from agentless_mcp.util.errors import CacheLocked

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
        symbols=SymbolService(extractor),
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

        with sqlite3.connect(first.database) as connection:
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
        with sqlite3.connect(report.database) as connection:
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

    def test_a_commit_makes_the_receipt_stale_and_names_both_generations(
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
        assert f"cache: g:{indexed.generation} stale (repo g:{live})" in receipt
        assert "rerun agentless-mcp index or pass --no-cache" in receipt

    def test_a_stale_index_still_answers_from_live_content(self, make_git_repo, extractor):
        root = make_git_repo({"core.py": CORE})
        cache.build_index(root, extractor, tree_oid=_tree_oid(root))
        changed = CORE + "\n\ndef rebate(sku):\n    return 2\n"
        (root / "core.py").write_text(changed, encoding="utf-8")
        commit(root, "second")

        source = cache.open_source(root, extractor, tree_oid=_tree_oid(root))
        names = [symbol.name for symbol in source.symbols_for(changed, "python", "core.py")]

        assert "stale" in source.receipt
        assert "rebate" in names


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

        assert "stale" in source.receipt


class TestSingleWriter:
    def test_a_second_index_run_refuses_while_the_lock_is_held(self, repo, extractor):
        first = cache.build_index(repo, extractor)
        lock_path = first.database.parent / cache.LOCK_NAME
        handle = lock_path.open("w", encoding="utf-8")
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)

        try:
            with pytest.raises(CacheLocked, match="another index run holds the lock for"):
                cache.build_index(repo, extractor)
        finally:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
            handle.close()

        source = cache.open_source(repo, extractor, tree_oid=None)
        assert source.status().files == 3


class TestCommandLine:
    def test_index_summarises_what_it_did(self, repo, services, capsys):
        assert invoke(services, repo, "index") == EXIT_OK
        summary = capsys.readouterr().out.splitlines()[0]

        assert summary.startswith("indexed 3, reused 0, pruned 0, errors 0: 3 files, ")
        assert " imports, " in summary
        assert " refs at g:nogit:" in summary

    def test_a_second_index_run_reports_reuse(self, repo, services, capsys):
        invoke(services, repo, "index")
        capsys.readouterr()

        assert invoke(services, repo, "index") == EXIT_OK
        assert capsys.readouterr().out.startswith("indexed 0, reused 3, pruned 0, errors 0:")

    def test_force_reports_every_file_as_indexed(self, repo, services, capsys):
        invoke(services, repo, "index")
        capsys.readouterr()

        assert invoke(services, repo, "index", "--force") == EXIT_OK
        assert capsys.readouterr().out.startswith("indexed 3, reused 0, pruned 0, errors 0:")

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
        with sqlite3.connect(second.database) as connection:
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
