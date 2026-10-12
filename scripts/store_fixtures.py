"""Write the store crate's test fixtures: a tags.db that the Python indexer built, and read cases.

Development-time only. Run it from the repository root, in the project environment:

    uv run python scripts/store_fixtures.py

The script indexes a git copy of tests/characterization/fixtures/complexity_* with the project's own
agentless-mcp. The index runs with a temporary HOME and cache directory and a temporary copy of the
grammar libraries, so no user cache changes. Each read case records the answer of the Python cache
reader on a copy of that database, after the damage that the case names. With --check, the script
compares its output with the committed fixtures and writes nothing.
"""

import logging
import os
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile
from collections.abc import Callable, Iterator
from contextlib import closing, contextmanager
from dataclasses import dataclass
from functools import partial
from pathlib import Path, PurePosixPath
from typing import Any
from unittest import mock

import stdlib_fixtures
import tree_sitter_language_pack as pack
from fixture_files import Fixture, Identity, git_env, matches, run_git, write_or_check

from agentless_mcp.core import cache
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.fileparse import Unmeasured
from agentless_mcp.core.treewalk import RepoFile
from agentless_mcp.util import cachedir, filelock, platforms

REPO = Path(__file__).resolve().parents[1]
FIXTURES = REPO / "crates" / "store" / "fixtures"
DATABASE = FIXTURES / "tags.db"
SOURCES = REPO / "tests" / "characterization" / "fixtures"
AGENTLESS = Path(sys.executable).with_name("agentless-mcp")
ROOT = "/agentless-store-fixture/repo"
CREATED_AT = "2026-01-01T00:00:00+00:00"
INDEX_SECONDS = 300
STALE = "0" * 40
IDENTITY = Identity("fixture", "fixture@invalid", "2026-01-01T00:00:00Z")
# The stdlib generator writes files into this crate's fixture directory too.
SHARED = [stdlib_fixtures.CRATES / path for path in stdlib_fixtures.BUILDERS]

# A damage: a kind and its arguments. The Rust tests apply the same damage to their own copy.
Damage = list[Any]
NONE: Damage = ["none"]
GARBAGE = "this file is not a SQLite database\n" * 128

OPENS: list[tuple[str, Damage, str | None]] = [
    ("fresh", NONE, None),
    ("stale", NONE, STALE),
    ("bypassed", ["bypass"], None),
    ("missing", ["missing"], None),
    ("schema_15", ["sql", ["UPDATE meta SET schema_version = 15"]], None),
    ("schema_16", ["sql", ["UPDATE meta SET schema_version = 16"]], None),
    ("schema_18", ["sql", ["UPDATE meta SET schema_version = 18"]], None),
    ("other_repository", ["sql", ["UPDATE meta SET repo_root = '/elsewhere/repo'"]], None),
    ("no_meta_row", ["sql", ["DELETE FROM meta"]], None),
    ("no_meta_table", ["sql", ["DROP TABLE meta"]], None),
    ("garbage", ["bytes", GARBAGE], None),
    ("truncated", ["truncate", 8192], None),
    ("empty", ["truncate", 0], None),
    ("directory", ["directory"], None),
    ("locked", ["locked"], None),
]

DAMAGED_FILE = "complexity_py/clones.py"
ABSENT_FILE = "complexity_py/absent.py"
# A text edit of the damaged file: "append" adds a newline, "empty" reads an unindexed path.
LOOKUP_EDITS = [("digest", "append"), ("absent_path", "empty")]
# Each statement binds :file_id, the id of the damaged file.
LOOKUP_SQL = {
    "grammar": "UPDATE files SET grammar_version = '1.14.2' WHERE id = :file_id",
    "stamp": "UPDATE file_metrics SET stamp = '1/py2.7' WHERE file_id = :file_id",
    "language": "UPDATE file_metrics SET language = 'ruby' WHERE file_id = :file_id",
    "no_metrics_row": "DELETE FROM file_metrics WHERE file_id = :file_id",
    "blocks_reason": "UPDATE file_metrics SET blocks_reason = 'stored' WHERE file_id = :file_id",
    "count": "UPDATE file_metrics SET block_count = block_count + 1 WHERE file_id = :file_id",
    "no_block_rows": "DELETE FROM blocks WHERE file_id = :file_id",
    "unit_count_text": "UPDATE file_metrics SET unit_count = 'many' WHERE file_id = :file_id",
    "sloc_truncated": "UPDATE file_metrics SET sloc_lines = '[1, 2' WHERE file_id = :file_id",
    "sloc_float": "UPDATE file_metrics SET sloc_lines = '[1.5]' WHERE file_id = :file_id",
    "sloc_null": "UPDATE file_metrics SET sloc_lines = NULL WHERE file_id = :file_id",
    "sloc_integer": "UPDATE file_metrics SET sloc_lines = 7 WHERE file_id = :file_id",
    "sloc_long": (
        "UPDATE file_metrics SET sloc_lines = json_quote(hex(zeroblob(50))) "
        "WHERE file_id = :file_id"
    ),
    "start_line_text": "UPDATE blocks SET start_line = 'abc' WHERE file_id = :file_id",
    "start_line_underscore": "UPDATE blocks SET start_line = '1_0' WHERE file_id = :file_id",
    "start_line_real": "UPDATE blocks SET start_line = 2.5 WHERE file_id = :file_id",
    "function_text": "UPDATE blocks SET is_function = 'no' WHERE file_id = :file_id",
    "key_not_utf8": (
        "UPDATE blocks SET type2_key = CAST(X'41C328' AS TEXT) WHERE file_id = :file_id"
    ),
    "key_not_utf8_with_nul": (
        "UPDATE blocks SET type2_key = CAST(X'41C30042' AS TEXT) WHERE file_id = :file_id"
    ),
    "start_line_separator": "UPDATE blocks SET start_line = '\x1c5' WHERE file_id = :file_id",
}

RECEIPT_ROOTS = [
    ROOT,
    "/agentless-store-fixture/a b",
    "/agentless-store-fixture/it's",
    "/agentless-store-fixture/café",
    "/agentless-store-fixture/$HOME",
    '/agentless-store-fixture/say"x"',
]
KEY_CACHE_HOME = "/agentless-store-fixture/cache-home"
KEY_ROOTS = [
    ROOT,
    "/agentless-store-fixture/a b",
    "/agentless-store-fixture/café",
    "/agentless-store-fixture/日本",
]

# A manifest entry: a path, and its size and mtime in nanoseconds, or None for a missing file.
MANIFESTS: list[list[tuple[str, int | None, int | None]]] = [
    [],
    [("a.py", 3, 1_700_000_000_123_456_789)],
    [
        ("a.py", 3, 1_700_000_000_123_456_789),
        ("dir/b c.ts", 0, 0),
        ("café.rs", 10, -1_000_000_001),
        ("missing.go", None, None),
    ],
]

VALUES: list[object] = [
    None,
    0,
    -5,
    2**62,
    3.5,
    -3.9,
    0.0,
    -0.0,
    "abc",
    " 12 ",
    "1_000",
    "1__0",
    "+7",
    "-0",
    "",
    "x" * 300,
    "it's",
    "café",
    b"12",
    b"a'b",
    b"\x00\xff\t",
    b"",
    b" 5 ",
    "\x1c5",
    "5\x1f",
    "\x855",
    b"\x1c5",
    b"\xc2\xa05",
    b"\x0b5",
]

SLOC_VALUES: list[object] = [
    "[]",
    "[1, 2]",
    "[1, 2",
    "[1.5]",
    "[true]",
    "null",
    '{"a": 1}',
    None,
    7,
    2.5,
    b"[1]",
    '"' + "x" * 100 + '"',
    " [1] ",
    "[-0]",
    "[1e2]",
    "[99999999999999999999]",
    "[NaN]",
    "",
    "[\t1,\n2\r]",
    "\r\n[ 1 , -2 ]\t",
    "[ ]",
    "[\n]",
    "[\x0b1]",
    "[\xa01]",
    "[1]\x0c",
    "[-0, 0]",
    "[-]",
    "[+1]",
    "[01]",
    "[-01]",
    "[1.0]",
    "[1e5]",
    "[1E5]",
    "﻿[1]",
    "[﻿1]",
    "[1,]",
    "[1, ]",
    "[,1]",
    "[1 2]",
    "[1]]",
    "[\n1\n2]",
    "[[1]]",
    "[1, [2]]",
    "[" + "9" * 4300 + "]",
    "[-" + "9" * 4300 + "]",
    "[" + "9" * 4301 + "]",
    "[1, -" + "8" * 4301 + "]",
    "[" + ", ".join(["123"] * 2000) + "]",
]

DISCARDS = ["plain", "sidecars", "locked", "read_only", "unlink_refused"]
# The setups whose failure comes from a directory mode: the lock open, then the unlink.
MODE_SETUPS = ("read_only", "unlink_refused")


@dataclass(frozen=True)
class Reader:
    work: Path
    database: Path
    generation: str
    extractor: TreeSitterExtractor


def main() -> int:
    built: dict[str, bytes] = {}
    builders: dict[str, Callable[[], Fixture]] = {"cases": partial(cases_fixture, built)}
    code = write_or_check(__doc__.splitlines()[0], FIXTURES, builders, SHARED)
    if "database" not in built:
        return code
    return max(code, database_result(built["database"], check="--check" in sys.argv[1:]))


def database_result(content: bytes, *, check: bool) -> int:
    if not check:
        DATABASE.write_bytes(content)
        print(f"wrote {DATABASE}")
        return 0
    if matches(DATABASE, content):
        return 0
    print(f"stale: {DATABASE}", file=sys.stderr)
    return 1


def cases_fixture(built: dict[str, bytes]) -> Fixture:
    with tempfile.TemporaryDirectory(prefix="store-fixtures-") as name:
        work = Path(name)
        database = build_database(work)
        built["database"] = database.read_bytes()
        reader = Reader(work, database, generation_of(database), TreeSitterExtractor())
        return {
            "fixture": [[reader.generation, row_counts(database)]],
            "opens": [open_case(reader, *case) for case in OPENS],
            "lookups": lookup_cases(reader),
            "receipts": receipt_cases(reader.generation),
            "keys": [key_case(root) for root in KEY_ROOTS],
            "nogit": [nogit_case(work, entries) for entries in MANIFESTS],
            "values": [value_case(value) for value in VALUES],
            "sloc": [sloc_case(value) for value in SLOC_VALUES],
            "discards": [discard_case(reader, setup) for setup in DISCARDS],
        }


def build_database(work: Path) -> Path:
    grammars = work / "grammars"
    shutil.copytree(pack.cache_dir(), grammars)
    repo = work / "repo"
    for source in sorted(SOURCES.glob("complexity_*")):
        shutil.copytree(source, repo / source.name, ignore=shutil.ignore_patterns("__pycache__"))
    git(repo, "init", "-q", "-b", "main")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "Add the store sources", "-m", "The fixture indexes them.")
    run_index(work, repo, grammars)
    (indexed,) = (work / "cache" / "agentless-mcp").glob("*/tags.db")
    return normalized(indexed, work / "tags.db")


def git(repo: Path, *args: str) -> None:
    run_git(repo, IDENTITY, *args)


def run_index(work: Path, repo: Path, grammars: Path) -> None:
    env = {
        **git_env(IDENTITY, os.environ),
        "HOME": str(work / "home"),
        "XDG_CACHE_HOME": str(work / "cache"),
        "TREE_SITTER_LANGUAGE_PACK_CACHE_DIR": str(grammars),
        "AGENTLESS_MCP_NO_DOWNLOAD": "1",
        "AGENTLESS_MCP_NO_AUTO_WARM": "1",
        "AGENTLESS_MCP_NO_AUTO_INDEX": "1",
    }
    command = [str(AGENTLESS), "index", "--repo", str(repo)]
    done = subprocess.run(
        command, check=False, env=env, capture_output=True, text=True, timeout=INDEX_SECONDS
    )
    if done.returncode != 0:
        message = f"the index exited with {done.returncode}: {done.stderr}"
        raise SystemExit(message)


# The repository root and the creation time are the two values that differ between builds.
def normalized(indexed: Path, out: Path) -> Path:
    with closing(sqlite3.connect(indexed, isolation_level=None)) as connection:
        connection.execute("UPDATE meta SET repo_root = ?, created_at = ?", (ROOT, CREATED_AT))
        connection.execute("VACUUM INTO ?", (str(out),))
    with closing(sqlite3.connect(out, isolation_level=None)) as connection:
        connection.execute("PRAGMA journal_mode=WAL")
    return out


def generation_of(database: Path) -> str:
    with closing(sqlite3.connect(database)) as connection:
        meta = cache._read_meta(connection)
    if meta is None:
        message = f"{database} holds no meta row"
        raise SystemExit(message)
    return meta.generation


def row_counts(database: Path) -> list[int]:
    with closing(sqlite3.connect(database)) as connection:
        counts = cache._row_counts(connection)
    return [counts.files, counts.tags, counts.imports, counts.refs]


@contextmanager
def damaged(reader: Reader, damage: Damage, path: str = "") -> Iterator[Path]:
    directory = Path(tempfile.mkdtemp(dir=reader.work))
    copy = directory / "tags.db"
    shutil.copyfile(reader.database, copy)
    apply_damage(reader, copy, damage, path)
    try:
        yield copy
    finally:
        shutil.rmtree(directory)


def apply_damage(reader: Reader, copy: Path, damage: Damage, path: str) -> None:
    kind = damage[0]
    if kind == "sql":
        with closing(sqlite3.connect(copy, isolation_level=None)) as connection:
            for statement in damage[1]:
                connection.execute(statement, bound(connection, statement, path))
    elif kind == "bytes":
        copy.write_bytes(damage[1].encode("latin-1"))
    elif kind == "truncate":
        copy.write_bytes(reader.database.read_bytes()[: damage[1]])
    elif kind == "directory":
        copy.unlink()
        copy.mkdir()
    elif kind == "missing":
        copy.unlink()


def bound(connection: sqlite3.Connection, statement: str, path: str) -> dict[str, object]:
    if ":file_id" not in statement:
        return {}
    (file_id,) = connection.execute("SELECT id FROM files WHERE path = ?", (path,)).fetchone()
    return {"file_id": file_id}


@contextmanager
def exclusive_writer(copy: Path, *, held: bool) -> Iterator[None]:
    if not held:
        yield
        return
    with closing(sqlite3.connect(copy, isolation_level=None)) as connection:
        connection.execute("PRAGMA locking_mode=EXCLUSIVE")
        connection.execute("BEGIN EXCLUSIVE")
        connection.execute("UPDATE meta SET created_at = created_at WHERE id = 1")
        connection.execute("COMMIT")
        yield


def open_case(
    reader: Reader, name: str, damage: Damage, repo_generation: str | None
) -> list[object]:
    locked = damage[0] == "locked"
    tree_oid = repo_generation or reader.generation
    with (
        damaged(reader, damage) as copy,
        exclusive_writer(copy, held=locked),
        mock.patch.object(cache, "cache_path", return_value=copy),
        mock.patch.object(
            cache, "SQLITE_TIMEOUT_SECONDS", 0.0 if locked else cache.SQLITE_TIMEOUT_SECONDS
        ),
    ):
        source = cache.open_source(
            Path(ROOT), reader.extractor, tree_oid=tree_oid, no_cache=damage[0] == "bypass"
        )
        status = source.status()
        source.close()
        exists = copy.exists()
    return [name, damage, tree_oid, kind_of(source, status), status.receipt, status.note, exists]


def kind_of(source: cache.FileSource, status: cache.CacheStatus) -> str:
    if source.cached:
        return "usable"
    if not status.enabled:
        return "bypassed"
    return "refused" if status.note else "absent"


def lookup_cases(reader: Reader) -> list[object]:
    with closing(sqlite3.connect(reader.database)) as connection:
        paths = sorted(cache._load_entries(connection))
    plain = [lookup_case(reader, path, path, "none", NONE) for path in paths]
    edited = [
        lookup_case(reader, name, ABSENT_FILE if edit == "empty" else DAMAGED_FILE, edit, NONE)
        for name, edit in LOOKUP_EDITS
    ]
    damaged_cases = [
        lookup_case(reader, name, DAMAGED_FILE, "none", ["sql", [statement]])
        for name, statement in LOOKUP_SQL.items()
    ]
    return [*plain, *edited, *damaged_cases]


def lookup_case(reader: Reader, name: str, path: str, edit: str, damage: Damage) -> list[object]:
    language = TreeSitterExtractor.SUPPORTED_EXTENSIONS[PurePosixPath(path).suffix]
    text = source_text(path, edit)
    with damaged(reader, damage, path) as copy:
        source = cache._open_indexed(copy, Path(ROOT), reader.extractor, tree_oid=reader.generation)
        if not isinstance(source, cache.CachedSource):
            message = f"{name}: the damaged database was refused: {source.receipt}"
            raise SystemExit(message)
        try:
            outcome = lookup(source, path, text, language, copy)
        finally:
            source.close()
    return [name, path, language, edit, damage, outcome]


def source_text(path: str, edit: str) -> str:
    if edit == "empty":
        return ""
    text = (SOURCES / path).read_bytes().decode("utf-8")
    return text + "\n" if edit == "append" else text


def lookup(
    source: cache.CachedSource, path: str, text: str, language: str, copy: Path
) -> list[object]:
    warnings: list[str] = []
    read = partial(source._block_rows, language=language)
    with captured(warnings):
        rows = source._rows_or_none(read, source._fresh_file_id(text, path), path, "block")
    if rows is None:
        return ["unreadable", warnings[0].replace(str(copy), "{db}")] if warnings else ["miss"]
    if isinstance(rows, Unmeasured):
        return ["unmeasured", rows.reason]
    blocks = [
        [block.start_line, block.end_line, block.key, block.exact, block.function, block.tokens]
        for block in rows.blocks
    ]
    return ["hit", blocks]


class Collected(logging.Handler):
    def __init__(self, sink: list[str]) -> None:
        super().__init__(logging.WARNING)
        self.sink = sink

    def emit(self, record: logging.LogRecord) -> None:
        self.sink.append(record.getMessage())


@contextmanager
def captured(sink: list[str]) -> Iterator[None]:
    logger = logging.getLogger(cache.__name__)
    handler = Collected(sink)
    logger.addHandler(handler)
    logger.propagate = False
    try:
        yield
    finally:
        logger.propagate = True
        logger.removeHandler(handler)


def receipt_cases(generation: str) -> list[object]:
    return [
        receipt_case(generation, repo_generation, root)
        for root in RECEIPT_ROOTS
        for repo_generation in (generation, STALE)
    ]


def receipt_case(generation: str, repo_generation: str, root: str) -> list[object]:
    status = cache.CacheStatus(
        path=Path(KEY_CACHE_HOME) / "tags.db",
        generation=generation,
        repo_generation=repo_generation,
        generation_matches=generation == repo_generation,
        enabled=True,
        files=0,
        tags=0,
        note="",
        repo_root=Path(root),
    )
    return [generation, repo_generation, root, status.receipt, status.generation_matches]


def key_case(root: str) -> list[object]:
    with mock.patch.dict(os.environ, {"XDG_CACHE_HOME": KEY_CACHE_HOME}):
        return [str(cachedir.cache_root()), root, str(cache.cache_path(Path(root)))]


def nogit_case(work: Path, entries: list[tuple[str, int | None, int | None]]) -> list[object]:
    root = Path(tempfile.mkdtemp(dir=work))
    for path, size, mtime in entries:
        if size is not None and mtime is not None:
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(b"x" * size)
            os.utime(target, ns=(mtime, mtime))
    walked = [RepoFile(path=path, size=size or 0) for path, size, _ in entries]
    with mock.patch.object(cache, "walk_repo", return_value=walked):
        generation = cache.repo_generation(root, None)
    return [[manifest_row(root, path) for path, _, _ in entries], generation]


def manifest_row(root: Path, path: str) -> list[object]:
    target = root / path
    if not target.exists():
        return [path, None]
    found = target.stat()
    return [path, [found.st_size, found.st_mtime_ns]]


def cell(value: object) -> list[object]:
    if value is None:
        return ["null"]
    if isinstance(value, int):
        return ["integer", value]
    if isinstance(value, float):
        bits = struct.unpack("<Q", struct.pack("<d", value))[0]
        return ["real", f"0x{bits:016x}"]
    if isinstance(value, bytes):
        return ["blob", list(value)]
    return ["text", value]


def value_case(value: Any) -> list[object]:
    try:
        converted: list[object] = ["ok", int(value)]
    except (ValueError, TypeError) as error:
        converted = ["error", repr(error)]
    return [cell(value), converted, str(value), bool(value), repr(value)]


def sloc_case(value: object) -> list[object]:
    try:
        cache._sloc_from_row(value)
    except (ValueError, TypeError) as error:
        return [cell(value), repr(error)]
    return [cell(value), None]


@contextmanager
def held_lock(directory: Path, *, held: bool) -> Iterator[None]:
    if not held:
        yield
        return
    flavour = platforms.family(sys.platform)
    with filelock.exclusive(directory / cache.LOCK_NAME, flavour=flavour):
        yield


def discard_case(reader: Reader, setup: str) -> list[object]:
    directory = Path(tempfile.mkdtemp(dir=reader.work))
    copy = directory / "tags.db"
    shutil.copyfile(reader.database, copy)
    if setup == "sidecars":
        (directory / "tags.db-wal").write_bytes(b"")
        (directory / "tags.db-shm").write_bytes(b"")
    if setup == "unlink_refused":
        (directory / cache.LOCK_NAME).write_bytes(b"")
    warnings: list[str] = []
    with captured(warnings), held_lock(directory, held=setup == "locked"):
        if setup in MODE_SETUPS:
            directory.chmod(0o555)
        cache._discard_unlocked(copy, Path(ROOT))
        directory.chmod(0o700)
    remaining = sorted(entry.name for entry in directory.iterdir())
    shutil.rmtree(directory)
    shown = [warning.replace(str(copy), "{db}") for warning in warnings]
    return [setup, shown, remaining]


if __name__ == "__main__":
    sys.exit(main())
