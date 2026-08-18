"""SQLite tag cache keyed on file sha256; WAL, single writer.

The cache is an optimization and never a source of truth. Index-free
on-demand parsing answers every question this package is asked; the cache
only removes the symbol-extraction parse from that work when it can prove the
file it is answering about has not changed.

**Where it lives.** ``$XDG_CACHE_HOME/agentless-mcp/<key>/tags.db``, where the
key is the first 16 hex digits of the sha256 of the repository's realpath.
Never inside the analysed repository: the posture towards a target repo is
strictly read-only, and a workspace of seven repositories must not grow seven
gitignore entries.

**What is stored.** Extracted symbols, never parse trees -- the 30GB-resident
failure mode of tree-holding indexers is a design constraint here, not an
incident to react to. Imports and identifier references are *not* cached, so a
whole-repository scan still parses each file for those; the cache removes one
of the three parses a scan performs, and every single-file view (expand,
slice, find) gets its symbols without parsing at all.

**Freshness has two layers, and only one of them is load-bearing.**

* Per file, the digest. A cached row is used only when the sha256 of the text
  actually being answered about matches the sha256 the row was extracted from,
  and only when the grammar pack that produced it is still the installed one.
  A dirty worktree, a mid-rebase checkout or a file edited a second ago
  therefore re-extracts *that file* and cannot serve a wrong answer. This is
  the guarantee; it keys on the invariant (this content produced these
  symbols) rather than on a proxy for it.
* Per repository, the generation. ``meta.generation_tree_oid`` records the
  git tree OID (or, outside git, a ``nogit:`` digest over the sorted
  ``(path, size, mtime_ns)`` manifest) the index was built from. Comparing it
  to the live one tells a caller whether the index is worth what it cost --
  a stale generation means many files will re-extract on demand, not that the
  answer is wrong. It is reported in the receipt with the remediation, never
  silently.

**One writer.** An index run holds an ``flock`` on ``write.lock`` next to the
database for its whole duration and writes inside one ``BEGIN IMMEDIATE``
transaction. A second concurrent run fails immediately with
:class:`~agentless_mcp.util.errors.CacheLocked` naming the repository rather
than queueing behind the first. Readers never block: the database runs in WAL
mode, and a reader that finds no database, a corrupt one or one written by an
older schema simply reports ``cache: none`` and parses on demand.

``fcntl`` makes the writer POSIX-only; Windows support is Phase 4 in the plan.
"""

import fcntl
import hashlib
import json
import os
import sqlite3
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Protocol

from agentless_mcp.core import grammars
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.symbols import ASTSymbol, SymbolKind, qualname
from agentless_mcp.core.treewalk import walk_repo
from agentless_mcp.util.errors import CacheLocked, LanguageUnavailable
from agentless_mcp.util.fslimits import read_bounded

# Bumping this drops the database and rebuilds it. That is the whole migration
# policy: the file is derived data, so a schema change costs one re-index and
# never a migration script.
SCHEMA_VERSION = 1

ENV_CACHE_HOME = "XDG_CACHE_HOME"
APPLICATION_DIR = "agentless-mcp"
DATABASE_NAME = "tags.db"
LOCK_NAME = "write.lock"

# 64 bits of realpath digest: enough that two repositories on one machine do
# not collide, short enough that the directory name is still readable. The
# repository's own path is stored in ``meta`` and checked on open, so a
# collision degrades to "cache: none" rather than to a wrong answer.
KEY_LENGTH = 16

NOGIT_PREFIX = "nogit:"

# SQLite is out-of-process state like any other: a lock wait gets a bound.
SQLITE_TIMEOUT_SECONDS = 5.0

# Directories under the user's cache home hold derived facts about private
# repositories, so they are owner-only.
DIRECTORY_MODE = 0o700

RECEIPT_NONE = "none"
RECEIPT_BYPASSED = "bypassed (--no-cache)"
REMEDIATION = "rerun agentless-mcp index or pass --no-cache"


@dataclass(frozen=True)
class CacheStatus:
    """What the cache contributed to one answer, for the receipt and reports.

    ``generation`` is the generation the *index* was built at and ``repo``
    the one the repository is at now. They differ when the tree moved since
    the last index run, which is a cost statement rather than a correctness
    one: every row served is still digest-checked against the file it
    describes.
    """

    path: Path | None
    generation: str | None
    repo_generation: str | None
    fresh: bool
    enabled: bool
    files: int
    tags: int
    note: str

    @property
    def receipt(self) -> str:
        """Return the ``cache:`` field of the response receipt."""
        if not self.enabled:
            return RECEIPT_BYPASSED
        if self.generation is None:
            return f"{RECEIPT_NONE} ({self.note})" if self.note else RECEIPT_NONE
        if self.fresh:
            return f"g:{self.generation} fresh"
        return f"g:{self.generation} stale (repo g:{self.repo_generation}) - {REMEDIATION}"

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this status."""
        return {
            "receipt": self.receipt,
            "path": str(self.path) if self.path is not None else None,
            "generation": self.generation,
            "repo_generation": self.repo_generation,
            "fresh": self.fresh,
            "enabled": self.enabled,
            "files": self.files,
            "tags": self.tags,
            "note": self.note,
        }


class SymbolSource(Protocol):
    """Where a view gets a file's symbols from.

    The seam that keeps the cache invisible above :mod:`agentless_mcp.core`:
    a service asks for the symbols of a text it already holds and never learns
    whether they were parsed or read back from SQLite.
    """

    @property
    def receipt(self) -> str:
        """The ``cache:`` field describing this source."""
        ...

    def symbols_for(self, text: str, language: str, path: str) -> list[ASTSymbol]:
        """Return the symbols ``text`` defines, as the extractor would."""
        ...

    def status(self) -> CacheStatus:
        """Describe this source, including row counts when it has any."""
        ...


class OnDemandSource:
    """Parses every file it is asked about. The default and the fallback."""

    def __init__(self, extractor: TreeSitterExtractor, status: CacheStatus | None = None) -> None:
        self._extractor = extractor
        self._status = status if status is not None else _absent_status(None)

    @property
    def receipt(self) -> str:
        """The ``cache:`` field describing this source."""
        return self._status.receipt

    def symbols_for(self, text: str, language: str, path: str) -> list[ASTSymbol]:
        """Extract symbols from ``text`` with no cache involved."""
        return self._extractor.extract_from_source(text, language, path)

    def status(self) -> CacheStatus:
        """Describe why there is no cache behind this source."""
        return self._status


@dataclass(frozen=True)
class _FileEntry:
    """The digest and grammar version one indexed file was recorded with."""

    digest: str
    grammar_version: str


@dataclass(frozen=True)
class _Meta:
    """The single meta row: which schema, which repository, which generation."""

    schema_version: int
    repo_root: str
    generation: str
    head_sha: str | None


@dataclass(frozen=True)
class _CacheState:
    """Everything an opened cache knows before a single row is read."""

    database: Path
    entries: dict[str, _FileEntry]
    generation: str
    repo_generation: str
    grammar_version: str


class CachedSource:
    """Serves symbols from the tag cache, re-extracting anything that moved."""

    def __init__(
        self,
        connection: sqlite3.Connection,
        extractor: TreeSitterExtractor,
        state: _CacheState,
    ) -> None:
        self._connection = connection
        self._extractor = extractor
        self._state = state

    @property
    def receipt(self) -> str:
        """The ``cache:`` field describing this source."""
        return self.status().receipt

    def symbols_for(self, text: str, language: str, path: str) -> list[ASTSymbol]:
        """Return cached symbols when the row still describes ``text``.

        The digest check is per file and happens on every read, which is what
        makes a dirty worktree safe: an edited file misses, re-extracts and
        answers from its live content while the rest of the repository is
        still served from the index.
        """
        entry = self._state.entries.get(path)
        if entry is None or entry.grammar_version != self._state.grammar_version:
            return self._extractor.extract_from_source(text, language, path)
        if entry.digest != content_digest(text):
            return self._extractor.extract_from_source(text, language, path)
        return self._rows(path, entry.digest)

    def status(self) -> CacheStatus:
        """Describe the cache, counting its rows."""
        files, tags = _row_counts(self._connection)
        return CacheStatus(
            path=self._state.database,
            generation=self._state.generation,
            repo_generation=self._state.repo_generation,
            fresh=self._state.generation == self._state.repo_generation,
            enabled=True,
            files=files,
            tags=tags,
            note="",
        )

    def close(self) -> None:
        """Release the read connection."""
        self._connection.close()

    def _rows(self, path: str, digest: str) -> list[ASTSymbol]:
        """Rebuild one file's symbols from its tag rows, in extraction order."""
        cursor = self._connection.execute(
            "SELECT name, kind, start_line, end_line, signature, parent, docstring, "
            "decorators, bases, language, is_public, is_async "
            "FROM tags WHERE path = ? AND sha256 = ? ORDER BY ordinal",
            (path, digest),
        )
        return [_symbol_from_row(row, path) for row in cursor.fetchall()]

    def _counts(self) -> tuple[int, int]:
        """Return the number of indexed files and tag rows."""
        files = self._connection.execute("SELECT COUNT(*) FROM files").fetchone()
        tags = self._connection.execute("SELECT COUNT(*) FROM tags").fetchone()
        return int(files[0]), int(tags[0])


def effective_source(
    source: SymbolSource | None,
    extractor: TreeSitterExtractor,
) -> SymbolSource:
    """Return ``source``, or an on-demand source when a call carries none.

    Callers that never opened a cache -- the test suite, an embedding library
    user -- get the index-free path without having to construct anything.
    """
    return source if source is not None else OnDemandSource(extractor)


def cache_root() -> Path:
    """Return the directory holding every repository's cache, per XDG."""
    configured = os.environ.get(ENV_CACHE_HOME, "").strip()
    home = Path(configured) if configured else Path.home() / ".cache"
    return home / APPLICATION_DIR


def cache_path(repo_root: Path) -> Path:
    """Return the database path for one repository."""
    key = hashlib.sha256(str(repo_root.resolve()).encode("utf-8")).hexdigest()[:KEY_LENGTH]
    return cache_root() / key / DATABASE_NAME


def content_digest(text: str) -> str:
    """Return the sha256 a file's symbols are keyed on.

    Over the decoded text rather than the raw bytes on purpose: a reader holds
    the text a view was rendered from, the indexer holds the same decoding of
    the same file, and hashing what both sides actually have is what makes the
    two agree on files with undecodable bytes.
    """
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def repo_generation(root: Path, tree_oid: str | None) -> str:
    """Return the generation identifier for ``root``.

    Inside git that is the tree OID of HEAD. Outside it, a digest over the
    sorted ``(path, size, mtime_ns)`` manifest -- weaker than a content hash
    and cheaper than one, which is the right trade for a flag whose job is to
    say "this index is worth re-running", not to protect an answer.
    """
    if tree_oid:
        return tree_oid
    return NOGIT_PREFIX + _manifest_digest(root)


@dataclass(frozen=True)
class _OpenedIndex:
    """What an opened database holds, or why it must not be used.

    ``meta`` is None exactly when ``note`` says what was wrong, so the caller
    has one thing to test and one message to report.
    """

    meta: _Meta | None
    entries: dict[str, _FileEntry]
    note: str


def _read_index(connection: sqlite3.Connection, repo_root: Path) -> _OpenedIndex:
    """Read the meta row and the file digests, refusing an unusable database."""
    meta = _read_meta(connection)
    note = _rejection(meta, repo_root)
    if meta is None or note:
        return _OpenedIndex(meta=None, entries={}, note=note)
    return _OpenedIndex(meta=meta, entries=_load_entries(connection), note="")


def open_source(
    root: Path,
    extractor: TreeSitterExtractor,
    *,
    tree_oid: str | None,
    no_cache: bool = False,
) -> SymbolSource:
    """Open the tag cache for ``root``, degrading to on-demand parsing.

    Every failure mode -- no database, a database from an older schema, a
    corrupt file, a key collision with another repository -- produces an
    on-demand source carrying the reason in its receipt. A cache that cannot
    be trusted is not consulted, and the caller is told which happened.
    """
    resolved = root.resolve()
    database = cache_path(resolved)

    if no_cache:
        return OnDemandSource(extractor, _bypassed_status(database))
    if not database.exists():
        return OnDemandSource(extractor, _absent_status(database))

    try:
        connection = _connect(database)
    except sqlite3.DatabaseError as exc:
        _discard(database)
        return OnDemandSource(extractor, _absent_status(database, f"cache discarded: {exc}"))

    try:
        opened = _read_index(connection, resolved)
    except sqlite3.DatabaseError as exc:
        connection.close()
        _discard(database)
        return OnDemandSource(extractor, _absent_status(database, f"cache discarded: {exc}"))

    meta = opened.meta
    if meta is None:
        connection.close()
        _discard(database)
        return OnDemandSource(extractor, _absent_status(database, opened.note))

    return CachedSource(
        connection,
        extractor,
        _CacheState(
            database=database,
            entries=opened.entries,
            generation=meta.generation,
            repo_generation=repo_generation(resolved, tree_oid),
            grammar_version=grammars.pack_version(),
        ),
    )


@dataclass(frozen=True)
class IndexFailure:
    """One file the index run could not record, and why."""

    path: str
    reason: str


@dataclass(frozen=True)
class IndexReport:
    """What one index run did, in the numbers its summary line prints."""

    database: Path
    generation: str
    indexed: int
    reused: int
    pruned: int
    files: int
    tags: int
    failures: tuple[IndexFailure, ...]

    @property
    def errors(self) -> int:
        """How many files could not be recorded."""
        return len(self.failures)

    def summary_line(self) -> str:
        """Return the one-line summary the CLI prints."""
        return (
            f"indexed {self.indexed}, reused {self.reused}, pruned {self.pruned}, "
            f"errors {self.errors}: {self.files} files, {self.tags} tags "
            f"at g:{self.generation} in {self.database}"
        )

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report."""
        return {
            "database": str(self.database),
            "generation": self.generation,
            "indexed": self.indexed,
            "reused": self.reused,
            "pruned": self.pruned,
            "errors": self.errors,
            "files": self.files,
            "tags": self.tags,
            "failures": [{"path": entry.path, "reason": entry.reason} for entry in self.failures],
        }


@dataclass(frozen=True)
class _FileTags:
    """One file's recorded state: its digest, its size and its symbols."""

    path: str
    digest: str
    size: int
    language: str
    symbols: tuple[ASTSymbol, ...]


@dataclass(frozen=True)
class _IndexPlan:
    """The whole index run decided before a single row is written."""

    writes: tuple[_FileTags, ...]
    reused: tuple[str, ...]
    seen: frozenset[str]
    failures: tuple[IndexFailure, ...]


def build_index(
    root: Path,
    extractor: TreeSitterExtractor,
    *,
    tree_oid: str | None = None,
    head_sha: str | None = None,
    force: bool = False,
) -> IndexReport:
    """Build or refresh the tag cache for one repository.

    Reads the repository, decides everything, then writes once: the walk and
    the extraction happen outside the transaction, and the transaction only
    replays the decisions. Files whose ``(path, sha256)`` is unchanged since
    the last run keep their rows and are never re-extracted -- including the
    ones that define no symbols at all, which are recorded with their digest
    precisely so they are skipped next time.
    """
    resolved = root.resolve()
    database = cache_path(resolved)
    database.parent.mkdir(parents=True, exist_ok=True, mode=DIRECTORY_MODE)

    with _write_lock(database.parent, resolved):
        connection = _open_for_write(database)
        try:
            previous = _load_entries(connection)
            plan = _plan_index(resolved, extractor, previous=previous, force=force)
            generation = repo_generation(resolved, tree_oid)
            meta = _Meta(
                schema_version=SCHEMA_VERSION,
                repo_root=str(resolved),
                generation=generation,
                head_sha=head_sha,
            )
            _apply_plan(connection, plan, meta, previous)
            files, tags = _row_counts(connection)
        finally:
            connection.close()

    pruned = len([path for path in previous if path not in plan.seen])
    return IndexReport(
        database=database,
        generation=generation,
        indexed=len(plan.writes),
        reused=len(plan.reused),
        pruned=pruned,
        files=files,
        tags=tags,
        failures=plan.failures,
    )


def _plan_index(
    root: Path,
    extractor: TreeSitterExtractor,
    *,
    previous: dict[str, _FileEntry],
    force: bool,
) -> _IndexPlan:
    """Walk the repository and decide, per file, reuse or re-extract."""
    grammar_version = grammars.pack_version()
    writes: list[_FileTags] = []
    reused: list[str] = []
    seen: set[str] = set()
    failures: list[IndexFailure] = []

    for repo_file in walk_repo(root):
        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(Path(repo_file.path).suffix)
        if language is None:
            continue

        read = read_bounded(root / repo_file.path)
        if read.text is None:
            failures.append(IndexFailure(path=repo_file.path, reason=read.skipped or "unreadable"))
            continue

        seen.add(repo_file.path)
        digest = content_digest(read.text)
        entry = previous.get(repo_file.path)
        if not force and entry is not None and entry == _FileEntry(digest, grammar_version):
            reused.append(repo_file.path)
            continue

        try:
            symbols = extractor.extract_from_source(read.text, language, repo_file.path)
        except LanguageUnavailable as exc:
            seen.discard(repo_file.path)
            failures.append(IndexFailure(path=repo_file.path, reason=str(exc)))
            continue

        writes.append(
            _FileTags(
                path=repo_file.path,
                digest=digest,
                size=repo_file.size,
                language=language,
                symbols=tuple(symbols),
            )
        )

    return _IndexPlan(
        writes=tuple(writes),
        reused=tuple(reused),
        seen=frozenset(seen),
        failures=tuple(failures),
    )


def _apply_plan(
    connection: sqlite3.Connection,
    plan: _IndexPlan,
    meta: _Meta,
    previous: dict[str, _FileEntry],
) -> None:
    """Write the whole plan in one transaction, or none of it."""
    grammar_version = grammars.pack_version()
    vanished = [path for path in previous if path not in plan.seen]
    touched = [entry.path for entry in plan.writes]

    connection.execute("BEGIN IMMEDIATE")
    try:
        for path in (*vanished, *touched):
            connection.execute("DELETE FROM tags WHERE path = ?", (path,))
            connection.execute("DELETE FROM files WHERE path = ?", (path,))

        for entry in plan.writes:
            connection.execute(
                "INSERT INTO files (path, sha256, size, lang, grammar_version) "
                "VALUES (?, ?, ?, ?, ?)",
                (entry.path, entry.digest, entry.size, entry.language, grammar_version),
            )
            connection.executemany(
                "INSERT INTO tags (path, sha256, name, kind, is_def, start_line, end_line, "
                "signature, parent, qualname, docstring, decorators, bases, language, "
                "is_public, is_async, ordinal) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    _tag_row(entry.path, entry.digest, ordinal, symbol)
                    for ordinal, symbol in enumerate(entry.symbols)
                ],
            )

        connection.execute("DELETE FROM meta")
        connection.execute(
            "INSERT INTO meta (id, schema_version, repo_root, generation_tree_oid, head_sha, "
            "created_at) VALUES (1, ?, ?, ?, ?, ?)",
            (
                meta.schema_version,
                meta.repo_root,
                meta.generation,
                meta.head_sha,
                datetime.now(timezone.utc).isoformat(),
            ),
        )
    except BaseException:
        connection.rollback()
        raise
    connection.commit()


def _tag_row(path: str, digest: str, ordinal: int, symbol: ASTSymbol) -> tuple[Any, ...]:
    """Flatten one symbol into its tag row.

    ``is_def`` is 1 on every row today: the cache holds definitions only, and
    the column exists so that a reference-carrying row can be added later
    without a schema break for the readers that filter on it.
    """
    return (
        path,
        digest,
        symbol.name,
        symbol.kind.value,
        1,
        symbol.line_number,
        symbol.end_line_number,
        symbol.signature,
        symbol.parent_class,
        qualname(symbol),
        symbol.docstring,
        json.dumps(list(symbol.decorators)),
        json.dumps(list(symbol.bases)),
        symbol.language,
        int(symbol.is_public),
        int(symbol.is_async),
        ordinal,
    )


def _symbol_from_row(row: Sequence[Any], path: str) -> ASTSymbol:
    """Rebuild one symbol from its tag row.

    Explicit conversions rather than a trusting unpack: these rows are our
    own data, but they came off disk and a row written by a build that
    predates a field is foreign data like any other.
    """
    end_line = row[3]
    return ASTSymbol(
        name=str(row[0]),
        kind=SymbolKind(str(row[1])),
        module_path=path,
        line_number=int(row[2]),
        end_line_number=None if end_line is None else int(end_line),
        signature=str(row[4]),
        docstring=str(row[6]),
        parent_class=str(row[5]),
        decorators=tuple(str(item) for item in json.loads(str(row[7]))),
        bases=tuple(str(item) for item in json.loads(str(row[8]))),
        language=str(row[9]),
        is_public=bool(row[10]),
        is_async=bool(row[11]),
    )


def _connect(database: Path) -> sqlite3.Connection:
    """Open ``database`` in WAL mode with a bounded lock wait.

    ``check_same_thread=False`` because a source is built and used inside one
    request that an async adapter may hand to a worker thread; the connection
    is never shared between requests.
    """
    connection = sqlite3.connect(
        database,
        timeout=SQLITE_TIMEOUT_SECONDS,
        isolation_level=None,
        check_same_thread=False,
    )
    connection.execute("PRAGMA journal_mode=WAL")
    connection.execute("PRAGMA synchronous=NORMAL")
    return connection


def _open_for_write(database: Path) -> sqlite3.Connection:
    """Open the database ready to write, replacing one that cannot be opened.

    A truncated or corrupt file is derived data that failed: discarding and
    rebuilding it is the whole recovery procedure, and it happens under the
    write lock so no second process is looking at the file while it goes.
    """
    connection = _connect(database)
    try:
        _ensure_schema(connection)
    except sqlite3.DatabaseError:
        connection.close()
        _discard(database)
        connection = _connect(database)
        _ensure_schema(connection)
    return connection


def _ensure_schema(connection: sqlite3.Connection) -> None:
    """Create the schema, dropping a database written by another version."""
    try:
        meta = _read_meta(connection)
    except sqlite3.DatabaseError:
        meta = None

    stale = meta is not None and meta.schema_version != SCHEMA_VERSION
    if stale:
        for table in ("tags", "files", "meta"):
            connection.execute(f"DROP TABLE IF EXISTS {table}")

    connection.executescript(_SCHEMA)


_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version INTEGER NOT NULL,
    repo_root TEXT NOT NULL,
    generation_tree_oid TEXT NOT NULL,
    head_sha TEXT,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL,
    lang TEXT NOT NULL,
    grammar_version TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tags (
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
    ordinal INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS tags_path_sha256 ON tags (path, sha256);
"""


def _read_meta(connection: sqlite3.Connection) -> _Meta | None:
    """Return the meta row, or None when there is not one to read."""
    try:
        row = connection.execute(
            "SELECT schema_version, repo_root, generation_tree_oid, head_sha FROM meta WHERE id = 1"
        ).fetchone()
    except sqlite3.OperationalError:
        # No meta table: an empty database file, or one from before the table
        # existed. Both mean "no usable index", which is not an error.
        return None

    if row is None:
        return None
    return _Meta(
        schema_version=int(row[0]),
        repo_root=str(row[1]),
        generation=str(row[2]),
        head_sha=None if row[3] is None else str(row[3]),
    )


def _rejection(meta: _Meta | None, repo_root: Path) -> str:
    """Return why this database must not be used, or an empty string."""
    if meta is None:
        return "cache discarded: no index has been built"
    if meta.schema_version != SCHEMA_VERSION:
        return (
            f"cache discarded: schema {meta.schema_version} predates {SCHEMA_VERSION}, "
            "rerun agentless-mcp index"
        )
    if meta.repo_root != str(repo_root):
        return f"cache discarded: it was built for {meta.repo_root}"
    return ""


def _load_entries(connection: sqlite3.Connection) -> dict[str, _FileEntry]:
    """Return every indexed file's digest and grammar version."""
    try:
        rows = connection.execute("SELECT path, sha256, grammar_version FROM files").fetchall()
    except sqlite3.OperationalError:
        return {}
    return {
        str(row[0]): _FileEntry(digest=str(row[1]), grammar_version=str(row[2])) for row in rows
    }


def _row_counts(connection: sqlite3.Connection) -> tuple[int, int]:
    """Return the number of indexed files and tag rows."""
    files = connection.execute("SELECT COUNT(*) FROM files").fetchone()
    tags = connection.execute("SELECT COUNT(*) FROM tags").fetchone()
    return int(files[0]), int(tags[0])


@contextmanager
def _write_lock(directory: Path, repo_root: Path) -> Iterator[None]:
    """Hold the index write lock, or refuse immediately."""
    lock_path = directory / LOCK_NAME
    handle = lock_path.open("w", encoding="utf-8")
    try:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            message = f"another index run holds the lock for {repo_root}"
            raise CacheLocked(message) from exc
        try:
            yield
        finally:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
    finally:
        handle.close()


def _manifest_digest(root: Path) -> str:
    """Digest the sorted (path, size, mtime_ns) manifest of a non-git tree."""
    digest = hashlib.sha256()
    for repo_file in walk_repo(root):
        try:
            stat = (root / repo_file.path).stat()
        except OSError:
            digest.update(f"{repo_file.path}\0missing\n".encode())
            continue
        digest.update(f"{repo_file.path}\0{stat.st_size}\0{stat.st_mtime_ns}\n".encode())
    return digest.hexdigest()[:KEY_LENGTH]


def _absent_status(database: Path | None, note: str = "") -> CacheStatus:
    """Status for a repository with no usable cache."""
    return CacheStatus(
        path=database,
        generation=None,
        repo_generation=None,
        fresh=False,
        enabled=True,
        files=0,
        tags=0,
        note=note,
    )


def _bypassed_status(database: Path) -> CacheStatus:
    """Status for a call that asked for on-demand parsing explicitly."""
    return CacheStatus(
        path=database,
        generation=None,
        repo_generation=None,
        fresh=False,
        enabled=False,
        files=0,
        tags=0,
        note="",
    )


def _discard(database: Path) -> None:
    """Delete a database this build cannot use, with its WAL sidecars.

    Deleting rather than ignoring: the file is derived data, and leaving an
    unusable one behind would make every later call pay the same open, fail
    the same check and report the same degradation.
    """
    for suffix in ("", "-wal", "-shm"):
        Path(str(database) + suffix).unlink(missing_ok=True)
