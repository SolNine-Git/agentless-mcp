"""Write the fixtures of agentless.db and the index command, each with the reference result.

Development-time only. Run it from the repository root, in the project environment, with the
grammar libraries copied to a scratch directory, so that no run reads or writes a user cache:

    TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy> uv run python scripts/store_index_fixtures.py

The store sections hold the eviction plans over synthetic entries, the ceiling over environment
values, the index report texts, the receipt cache field in each state, and the stderr warnings.
The app section holds index scenarios: the steps that build a repository, and the reference index
report after each run. With --check, the script compares its output and writes nothing.
"""

import argparse
import contextlib
import io
import json
import os
import sqlite3
import struct
import sys
import tempfile
import types
from collections.abc import Callable, Iterator
from pathlib import Path
from unittest import mock

import app_fixtures
from fixture_files import Fixture, refuse_real_caches, render, write_or_check_files
from store_fixtures import RECEIPT_ROOTS, captured

from agentless_mcp.adapters.cli import main as cli_main
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.symbol_service import in_metric_scope
from agentless_mcp.core import cache
from agentless_mcp.core.extractor import TreeSitterExtractor

REPO = Path(__file__).resolve().parents[1]
STORE = REPO / "crates" / "store" / "fixtures" / "index"
APP = REPO / "crates" / "app" / "fixtures" / "index"
NOW = 1_800_000_000.0
HOUR = 3600.0
CACHE = "/cache"
DATABASE = "/cache/0123456789abcdef/agentless.db"
GENERATION = "0123abcd"
STALE = "fedcba98"
RANDOM_PLANS = 200

# (name, [(entry, size, hours before NOW)], limit, keep)
PLANS: list[tuple[str, list[tuple[str, int, float]], int, str]] = [
    ("empty", [], 0, "none"),
    ("under_the_limit", [("a", 10, 48), ("b", 10, 72)], 100, "none"),
    ("cascade", [("new", 10, 48), ("a", 50, 72), ("b", 1, 96), ("c", 1, 120)], 55, "none"),
    ("strict_limit_kept", [("new", 10, 48), ("a", 45, 72)], 55, "none"),
    ("strict_limit_over", [("new", 10, 48), ("a", 45, 72)], 54, "none"),
    ("protected_counts", [("new", 10, 0), ("recent", 40, 12), ("cold", 10, 48)], 55, "none"),
    ("boundary_protected", [("new", 10, 0), ("edge", 10, 24)], 0, "none"),
    ("just_past_boundary", [("new", 10, 0), ("edge", 10, 24.001)], 0, "none"),
    ("keep_spared", [("new", 100, 48), ("kept", 100, 72), ("old", 1, 96)], 10, "kept"),
    ("newest_spared", [("new", 1000, 48), ("old", 1, 96)], 10, "none"),
    ("ties_keep_listing_order", [("new", 1, 48), ("x", 10, 72), ("y", 10, 72)], 15, "none"),
    ("zero_limit", [("new", 1, 48), ("a", 1, 72), ("b", 1, 96)], 0, "none"),
]

CEILINGS: list[str] = [
    "",
    "   ",
    "0",
    "-0",
    "+0",
    "5",
    " 12 ",
    "1_000",
    "1__0",
    "+7",
    "-1",
    "1.5",
    "abc",
    "5 5",
    "\x1c5",
    "5\x1f",
    "\x855",
    "\N{NO-BREAK SPACE}5",
    "9" * 30,
    "9" * 4300,
    "9" * 4301,
    "1_" + "0" * 4299,
    "\udcff5",
    " \udcff ",
]
# The reference reads these digits; the Rust build refuses a digit outside ASCII (facts D4).
DEVIATIONS: list[str] = ["\N{ARABIC-INDIC DIGIT THREE}", "1\N{ARABIC-INDIC DIGIT THREE}"]

# (name, indexed, reused, pruned, files, tags, imports, refs, failures, skips, evicted)
REPORTS: list[tuple[str, int, int, int, int, int, int, int, int, int, tuple[int, int]]] = [
    ("empty", 0, 0, 0, 0, 0, 0, 0, 0, 0, (0, 0)),
    ("plain", 3, 2, 1, 5, 40, 7, 300, 0, 0, (0, 0)),
    ("one_eviction", 1, 0, 0, 1, 2, 0, 9, 0, 0, (1, 4096)),
    ("evictions", 1, 0, 0, 1, 2, 0, 9, 0, 0, (3, 123_456_789)),
    ("failures_and_skips", 2, 0, 0, 2, 5, 1, 30, 1, 2, (0, 0)),
    ("at_the_row_cap", 0, 0, 0, 0, 0, 0, 0, 10, 10, (0, 0)),
    ("over_the_row_cap", 0, 0, 0, 0, 0, 0, 0, 11, 25, (2, 10)),
]

PYTHON_TEXT = "import os\n\n\nclass Box:\n    def size(self, x):\n        return os.sep + x\n"
SCRIPT_TEXT = "export function add(a, b) {\n  return a + b;\n}\n"

SCENARIOS: list[dict[str, object]] = [
    {
        "name": "edits_in_git",
        "steps": [
            *app_fixtures.repo(
                {
                    "a.py": PYTHON_TEXT,
                    "web/b.js": SCRIPT_TEXT,
                    "notes.md": "# no grammar\n",
                    ".agentless-mcp.json": "{}",
                }
            ),
            ["index", "repo"],
            ["index", "repo"],
            ["write", "repo/a.py", "x = 1\n"],
            ["index", "repo"],
            ["remove", "repo/web/b.js"],
            ["index", "repo"],
            ["index_force", "repo"],
        ],
    },
    {
        "name": "skips_and_failures",
        "steps": [
            *app_fixtures.repo({"a.py": PYTHON_TEXT, "locked.py": "y = 2\n"}),
            ["repeat", "repo/big.py", "x", 1_000_001],
            ["bytes", "repo/latin.py", "s = '\N{LATIN SMALL LETTER Y WITH DIAERESIS}'\n"],
            ["chmod", "repo/locked.py", 0],
            ["index", "repo"],
            ["chmod", "repo/locked.py", 0o644],
            ["index", "repo"],
            ["remove", "repo/big.py"],
            ["index", "repo"],
        ],
    },
    {
        "name": "data_and_tests",
        "steps": [
            *app_fixtures.repo(
                {
                    "tests/test_a.py": "def test_a():\n    assert True\n",
                    "d.json": '{"k": [1, 2]}\n',
                    "e.yaml": "key: value\nlist:\n  - 1\n",
                    "f.toml": "[table]\nkey = 1\n",
                    "g.go": "package g\n\nfunc F() {}\n",
                    "broken.py": "def broken(:\n    pass\n",
                }
            ),
            ["index", "repo"],
        ],
    },
    {
        # Pinned modification times make the generation outside git, a manifest digest, repeat.
        "name": "outside_git",
        "steps": [
            ["write", "repo/a.py", PYTHON_TEXT],
            ["write", "repo/b.js", SCRIPT_TEXT],
            ["mtime", "repo/a.py", "2000-01-01T00:00:00Z"],
            ["mtime", "repo/b.js", "2001-02-03T04:05:06Z"],
            ["index", "repo"],
            ["write", "repo/b.js", "export const b = 1;\n"],
            ["mtime", "repo/b.js", "2002-03-04T05:06:07Z"],
            ["index", "repo"],
        ],
    },
]


def main() -> int:
    refuse_real_caches()
    return write_or_check_files(__doc__.splitlines()[0], STORE, build, [APP])


def build() -> dict[Path, bytes]:
    return {
        STORE / "eviction.json": render({"plans": plan_rows(), "ceilings": ceiling_rows()}),
        STORE / "report.json": render({"reports": [report_row(*row) for row in REPORTS]}),
        STORE / "receipts.json": render({"receipts": receipt_rows()}),
        STORE / "warnings.json": render(warning_rows()),
        APP / "scenarios.json": render({"scenarios": [scenario_row(s) for s in SCENARIOS]}),
    }


def bits(value: float) -> str:
    return f"0x{struct.unpack('<Q', struct.pack('<d', value))[0]:016x}"


def database_of(name: str) -> Path:
    return Path(CACHE) / name / "agentless.db"


def plan_row(name: str, entries: list[tuple[str, int, float]], limit: int, keep: str) -> list:
    found = [
        cache._CacheEntry(database_of(entry), size, NOW - hours * HOUR)
        for entry, size, hours in entries
    ]
    victims = cache._eviction_plan(found, limit, keep=database_of(keep), now=NOW)
    pairs = zip(entries, found, strict=True)
    shown = [[entry, size, bits(item.used_at)] for (entry, size, _), item in pairs]
    planned = [victim.database.parent.name for victim in victims]
    return [name, shown, limit, keep, bits(NOW), planned]


def draws(seed: int) -> Iterator[int]:
    state = seed
    while True:
        state = (state * 6364136223846793005 + 1442695040888963407) % 2**64
        yield state >> 33


def random_plan(number: int, draw: Iterator[int]) -> list:
    count = next(draw) % 8
    # Whole and half hours, so that some times fall on the 24-hour boundary and some tie.
    entries = [(f"d{index}", next(draw) % 101, (next(draw) % 145) / 2) for index in range(count)]
    keep = entries[next(draw) % count][0] if count and next(draw) % 2 else "none"
    return plan_row(f"random_{number}", entries, next(draw) % 401, keep)


def plan_rows() -> list[object]:
    draw = draws(2026)
    named = [plan_row(*plan) for plan in PLANS]
    return [*named, *(random_plan(number, draw) for number in range(RANDOM_PLANS))]


def ceiling_row(value: str | None) -> list[object]:
    warnings: list[str] = []
    environment = {} if value is None else {cache.ENV_MAX_CACHE_BYTES: value}
    with mock.patch.dict(os.environ, environment), captured(warnings):
        if value is None:
            os.environ.pop(cache.ENV_MAX_CACHE_BYTES, None)
        limit = cache.max_cache_bytes()
    shown = None if limit is None else str(limit)
    encoded = None if value is None else list(os.fsencode(value))
    return [encoded, shown, warnings[0] if warnings else None]


def ceiling_rows() -> list[object]:
    rows = [[*ceiling_row(value), None] for value in [None, *CEILINGS]]
    deviations = [[*ceiling_row(value), "deviation"] for value in DEVIATIONS]
    return [*rows, *deviations]


def report_of(row: tuple) -> cache.IndexReport:
    _, indexed, reused, pruned, files, tags, imports, refs, failures, skips, evicted = row
    return cache.IndexReport(
        database=Path(DATABASE),
        generation=GENERATION,
        indexed=indexed,
        reused=reused,
        pruned=pruned,
        files=files,
        tags=tags,
        imports=imports,
        refs=refs,
        failures=tuple(cache.IndexFailure(f"f{i}.py", f"reason {i}") for i in range(failures)),
        skipped_files=tuple(cache.IndexSkip(f"s{i}.py", f"skip {i}") for i in range(skips)),
        evicted=cache.EvictionReport(databases=evicted[0], size=evicted[1]),
    )


# The command's text form and exit code, from the reference command with the run replaced.
def text_of(report: cache.IndexReport) -> tuple[str, int]:
    args = argparse.Namespace(json=False, force=False, repo=None)
    context = types.SimpleNamespace(root=Path("/repo"), tree_oid=None, head_sha=None)
    services = types.SimpleNamespace(extractor=None, counter=None)
    out = io.StringIO()
    with (
        mock.patch.object(cli_main, "_resolve", return_value=context),
        mock.patch.object(cache, "build_index", return_value=report),
        contextlib.redirect_stdout(out),
    ):
        code = cli_main._cmd_index(args, services)
    return out.getvalue(), code


VALUE_NAMES = ("indexed", "reused", "pruned", "files", "tags", "imports", "refs")


def report_row(*row: object) -> dict[str, object]:
    report = report_of(row)
    text, code = text_of(report)
    values = dict(zip(VALUE_NAMES, row[1:8], strict=True))
    return {
        "name": row[0],
        "values": {**values, "failures": row[8], "skips": row[9], "evicted": list(row[10])},
        "summary": report.summary_line(),
        "json": json.dumps(report.as_dict()),
        "text": text,
        "code": code,
    }


# (kind, arguments) of each absent note, and the note the reference build gives for it.
NOTES: list[tuple[str, list[object], str]] = [
    ("no_index", [], cache.MESSAGES.cache_discarded_no_index),
    ("schema", [15, 17], cache.MESSAGES.cache_discarded_old_schema.format(found=15, expected=17)),
    (
        "other_repository",
        ["/elsewhere/repo"],
        cache.MESSAGES.cache_discarded_other_repo.format(repo_root="/elsewhere/repo"),
    ),
    ("unreadable", ["disk I/O error"], "cache unreadable: disk I/O error"),
    ("corrupt", ["file is not a database"], "cache discarded: file is not a database"),
]


def receipt(status: cache.CacheStatus, refreshing: bool) -> str:
    with mock.patch.object(cache, "auto_index_in_progress", return_value=refreshing):
        return status.receipt


def absent_rows(refreshing: bool) -> list[object]:
    plain = cache._absent_status(Path(DATABASE))
    rows = [["absent", None, [], refreshing, receipt(plain, refreshing)]]
    for kind, arguments, note in NOTES:
        status = cache._absent_status(Path(DATABASE), note)
        rows.append(["absent", kind, arguments, refreshing, receipt(status, refreshing)])
    bypassed = cache._bypassed_status(Path(DATABASE))
    rows.append(["bypassed", None, [], refreshing, receipt(bypassed, refreshing)])
    return rows


def indexed_row(root: str, repo_generation: str, refreshing: bool) -> list[object]:
    status = cache.CacheStatus(
        path=Path(DATABASE),
        generation=GENERATION,
        repo_generation=repo_generation,
        generation_matches=repo_generation == GENERATION,
        enabled=True,
        files=0,
        tags=0,
        note="",
        repo_root=Path(root),
    )
    return ["indexed", root, [GENERATION, repo_generation], refreshing, receipt(status, refreshing)]


def receipt_rows() -> list[object]:
    rows: list[object] = []
    for refreshing in (False, True):
        rows.extend(absent_rows(refreshing))
        rows.extend(
            indexed_row(root, repo_generation, refreshing)
            for root in RECEIPT_ROOTS
            for repo_generation in (GENERATION, STALE)
        )
    return rows


def warned(action: Callable[[], object]) -> list[str]:
    warnings: list[str] = []
    with captured(warnings):
        action()
    return warnings


def raising(error: Exception) -> Callable[..., object]:
    def fail(*_arguments: object, **_keywords: object) -> object:
        raise error

    return fail


def denied(errno: int) -> OSError:
    return PermissionError(errno, os.strerror(errno))


VICTIM = Path("/c/agentless-mcp/0123456789abcdef/agentless.db")


def restricting_rows() -> list[object]:
    directory = VICTIM.parent
    with (
        mock.patch.object(Path, "mkdir", return_value=None),
        mock.patch.object(Path, "chmod", raising(denied(1))),
    ):
        texts = warned(lambda: cache._ensure_cache_directory(directory))
    paths = [str(directory.parent), str(directory)]
    return [[path, 1, text] for path, text in zip(paths, texts, strict=True)]


def listing_rows() -> list[object]:
    root = VICTIM.parent.parent
    with mock.patch.object(Path, "iterdir", raising(denied(13))):
        texts = warned(lambda: cache._cache_entries(root))
    return [[str(root), 13, text] for text in texts]


def weighing_rows() -> list[object]:
    def stat(path: Path, **_keywords: object) -> os.stat_result:
        if str(path).endswith("-wal"):
            raise denied(13)
        raise FileNotFoundError(2, "No such file or directory")

    with mock.patch.object(Path, "stat", stat):
        texts = warned(lambda: cache._entry_bytes(VICTIM))
    return [[str(VICTIM), "-wal", 13, text] for text in texts]


def recheck_rows() -> list[object]:
    entry = cache._CacheEntry(VICTIM, 1, 0.0)
    with mock.patch.object(Path, "stat", raising(denied(13))):
        texts = warned(lambda: cache._evict_victim(entry, now=NOW))
    return [[str(VICTIM), 13, text] for text in texts]


def eviction_rows() -> list[object]:
    entry = cache._CacheEntry(VICTIM, 1, 0.0)
    with (
        mock.patch.object(cache, "_cache_entries", return_value=[entry]),
        mock.patch.object(cache, "_eviction_plan", return_value=[entry]),
        mock.patch.object(cache, "_write_lock", raising(denied(13))),
        mock.patch.dict(os.environ, {cache.ENV_MAX_CACHE_BYTES: "1"}),
    ):
        texts = warned(lambda: cache.enforce_cache_limit(keep=Path("/none")))
    return [[str(VICTIM), 13, text] for text in texts]


def reclaim_rows() -> list[object]:
    message = "database or disk is full"
    connection = types.SimpleNamespace(execute=raising(sqlite3.OperationalError(message)))
    texts = warned(lambda: cache._reclaim(connection))
    return [["OperationalError", message, text] for text in texts]


def row_rows() -> list[object]:
    state = cache._CacheState(VICTIM, {}, GENERATION, GENERATION, "", Path("/repo"))
    source = cache.CachedSource(None, None, state)
    message = "database disk image is malformed"
    read = raising(sqlite3.DatabaseError(message))
    rows: list[object] = []
    for kind in ("symbol", "block"):
        texts = warned(lambda kind=kind: source._rows_or_none(read, 1, "a.py", kind))
        rows.extend([str(VICTIM), kind, "a.py", "DatabaseError", message, text] for text in texts)
    return rows


def warning_rows() -> Fixture:
    return {
        "restricting": restricting_rows(),
        "listing": listing_rows(),
        "weighing": weighing_rows(),
        "recheck": recheck_rows(),
        "eviction": eviction_rows(),
        "reclaim": reclaim_rows(),
        "rows": row_rows(),
    }


REPORT_KEYS = (
    "indexed",
    "reused",
    "pruned",
    "skipped",
    "errors",
    "files",
    "tags",
    "imports",
    "refs",
)


def index_result(sandbox: Path, place: str, force: bool) -> dict[str, object]:
    context = resolve_repo(sandbox / place, None)
    git = cache.GitState(tree_oid=context.tree_oid, head_sha=context.head_sha)
    report = cache.build_index(
        context.root, TreeSitterExtractor(), measure=in_metric_scope, git=git, force=force
    )
    document = report.as_dict()
    return {
        **{key: document[key] for key in REPORT_KEYS},
        "generation": report.generation,
        "failures": [[failure.path, failure.reason] for failure in report.failures],
        "skipped_files": [[skip.path, skip.reason] for skip in report.skipped_files],
    }


def scenario_step(sandbox: Path, step: list[object]) -> dict[str, object] | None:
    kind, place = str(step[0]), str(step[1])
    if kind in ("index", "index_force"):
        return index_result(sandbox, place, kind == "index_force")
    if kind == "chmod":
        (sandbox / place).chmod(int(str(step[2])))
        return None
    app_fixtures.apply(sandbox, step)
    return None


# The modes go back to owner read and write, so that the temporary directory can be removed.
def scenario_row(scenario: dict[str, object]) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="store-index-fixture-") as made:
        sandbox = Path(made).resolve()
        with app_fixtures.isolated(sandbox):
            runs = [scenario_step(sandbox, step) for step in scenario["steps"]]
        for path in sandbox.rglob("*"):
            if not path.is_symlink():
                path.chmod(0o700 if path.is_dir() else 0o600)
    return {**scenario, "runs": [run for run in runs if run is not None]}


if __name__ == "__main__":
    sys.exit(main())
