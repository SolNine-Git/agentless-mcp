"""Write the store crate's test fixtures: the read cases of the Python cache reader.

Development-time only. Run it from the repository root, in the project environment:

    uv run python scripts/store_fixtures.py

The script indexes a git copy of tests/characterization/fixtures/complexity_* with the project's own
agentless-mcp into a temporary tags.db. The index runs with a temporary HOME and cache directory and
a temporary copy of the grammar libraries, so no user cache changes. Each lookup case records the
answer of the Python cache reader on that database. With --check, the script compares its output
with the committed fixtures and writes nothing.
"""

import logging
import os
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile
from collections.abc import Iterator
from contextlib import closing, contextmanager
from dataclasses import dataclass
from functools import partial
from pathlib import Path, PurePosixPath
from typing import Any
from unittest import mock

import stdlib_fixtures
import tree_sitter_language_pack as pack
from fixture_files import Fixture, Identity, git_env, run_git, write_or_check

from agentless_mcp.core import cache
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.fileparse import Unmeasured
from agentless_mcp.core.treewalk import RepoFile
from agentless_mcp.util import cachedir, filelock, platforms

REPO = Path(__file__).resolve().parents[1]
FIXTURES = REPO / "crates" / "store" / "fixtures"
SOURCES = REPO / "tests" / "characterization" / "fixtures"
AGENTLESS = Path(sys.executable).with_name("agentless-mcp")
ROOT = "/agentless-store-fixture/repo"
CREATED_AT = "2026-01-01T00:00:00+00:00"
INDEX_SECONDS = 300
STALE = "0" * 40
IDENTITY = Identity("fixture", "fixture@invalid", "2026-01-01T00:00:00Z")
# The stdlib generator writes files into this crate's fixture directory too.
SHARED = [stdlib_fixtures.CRATES / path for path in stdlib_fixtures.BUILDERS]

EDITED_FILE = "complexity_py/clones.py"
ABSENT_FILE = "complexity_py/absent.py"
# A text edit: "append" adds a newline to the edited file, "empty" reads an unindexed path.
LOOKUP_EDITS = [("digest", "append"), ("absent_path", "empty")]

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
    return write_or_check(__doc__.splitlines()[0], FIXTURES, {"cases": cases_fixture}, SHARED)


def cases_fixture() -> Fixture:
    with tempfile.TemporaryDirectory(prefix="store-fixtures-") as name:
        work = Path(name)
        database = build_database(work)
        reader = Reader(work, database, generation_of(database), TreeSitterExtractor())
        return {
            "lookups": lookup_cases(reader),
            "receipts": receipt_cases(reader.generation),
            "keys": [key_case(root) for root in KEY_ROOTS],
            "nogit": [nogit_case(work, entries) for entries in MANIFESTS],
            "values": [value_case(value) for value in VALUES],
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
    return out


def generation_of(database: Path) -> str:
    with closing(sqlite3.connect(database)) as connection:
        meta = cache._read_meta(connection)
    if meta is None:
        message = f"{database} holds no meta row"
        raise SystemExit(message)
    return meta.generation


def lookup_cases(reader: Reader) -> list[object]:
    with closing(sqlite3.connect(reader.database)) as connection:
        paths = sorted(cache._load_entries(connection))
    plain = [lookup_case(reader, path, path, "none") for path in paths]
    edited = [
        lookup_case(reader, name, ABSENT_FILE if edit == "empty" else EDITED_FILE, edit)
        for name, edit in LOOKUP_EDITS
    ]
    return [*plain, *edited]


def lookup_case(reader: Reader, name: str, path: str, edit: str) -> list[object]:
    language = TreeSitterExtractor.SUPPORTED_EXTENSIONS[PurePosixPath(path).suffix]
    text = source_text(path, edit)
    source = cache._open_indexed(
        reader.database, Path(ROOT), reader.extractor, tree_oid=reader.generation
    )
    if not isinstance(source, cache.CachedSource):
        message = f"{name}: the database was refused: {source.receipt}"
        raise SystemExit(message)
    try:
        outcome = lookup(source, path, text, language, reader.database)
    finally:
        source.close()
    return [name, path, language, edit, outcome]


def source_text(path: str, edit: str) -> str:
    if edit == "empty":
        return ""
    text = (SOURCES / path).read_bytes().decode("utf-8")
    return text + "\n" if edit == "append" else text


def lookup(
    source: cache.CachedSource, path: str, text: str, language: str, database: Path
) -> list[object]:
    warnings: list[str] = []
    read = partial(source._block_rows, language=language)
    with captured(warnings):
        rows = source._rows_or_none(read, source._fresh_file_id(text, path), path, "block")
    if rows is None:
        return ["unreadable", warnings[0].replace(str(database), "{db}")] if warnings else ["miss"]
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
