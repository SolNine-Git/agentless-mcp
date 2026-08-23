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

**What is stored.** Extracted facts -- symbols, import statements and
identifier references -- never parse trees; the 30GB-resident failure mode of
tree-holding indexers is a design constraint here, not an incident to react
to. All three are keyed the same way and gated by the same digest, so a fresh
index removes every parse a repository scan would perform and every
single-file view (expand, slice, find) gets its symbols without parsing at
all.

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
  a generation mismatch means changed files re-extract on demand, not that
  the answer is wrong. It is reported in the receipt as a performance
  condition, never as stale output.

**One writer.** An index run holds an exclusive lock on ``write.lock`` next to
the database for its whole duration and writes inside one ``BEGIN IMMEDIATE``
transaction. A second concurrent run fails immediately with
:class:`~agentless_mcp.util.errors.CacheLocked` naming the repository rather
than queueing behind the first. Readers never block: the database runs in WAL
mode, and a reader that finds no database, a corrupt one or one written by an
older schema simply reports ``cache: none`` and parses on demand. The lock
primitive itself lives in :mod:`agentless_mcp.util.filelock`, which is where
the POSIX and Windows implementations of "exclusive or refuse" are chosen
between.
"""

import hashlib
import json
import logging
import os
import sqlite3
import sys
import threading
import time
from collections.abc import Callable, Iterator, Sequence
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Protocol

from agentless_mcp.core import grammars
from agentless_mcp.core.extractor import IdentifierRole, Ref, TreeSitterExtractor
from agentless_mcp.core.imports import ImportStatement
from agentless_mcp.core.symbols import ASTSymbol, Rationale, SymbolKind, disambiguate
from agentless_mcp.core.treewalk import walk_repo
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util import filelock, platforms
from agentless_mcp.util.errors import AgentlessError, CacheLocked
from agentless_mcp.util.fslimits import DEFAULT_MAX_FILE_BYTES, read_bounded

# Bumping this drops the database and rebuilds it. That is the whole migration
# policy: the file is derived data, so a schema change costs one re-index and
# never a migration script.
# 2 (2026-08-18, Phase 4): imports and refs tables added beside tags.
# 3 (2026-08-19, Phase 8): Go methods carry their receiver type as their
#   parent, so the qualified names and stable ids stored in v2 rows are wrong
#   for every Go file. The per-file sha256 gate cannot catch this one -- the
#   content did not change, the extraction did -- so the version bump is the
#   only thing standing between an existing index and a repository map full
#   of ids that no longer address anything.
# 4 (2026-08-19): refs rows carry ``locally_bound``. v3 rows would rehydrate
#   every ref unflagged, and the resolver would go back to pointing a
#   function's own parameter names at unrelated repository symbols.
# 5 (2026-08-19): tag rows carry rationale nodes extracted from comments.
#   A v4 cache would otherwise make an indexed map disagree with an on-demand
#   map for unchanged files, which the content digest cannot detect.
# 6 (2026-08-19): reference rows carry an identifier role rather than a
#   parameter-only boolean. Reusing v5 rows would promote assignments,
#   attributes and keyword labels back into graph edges.
# 7 (2026-08-19): module-attribute rows carry their syntactic qualifier, so
#   ``core.helper`` can resolve through ``core`` without treating
#   ``sys.stderr`` as repository-wide name evidence.
# 8 (2026-08-23): import rows carry ``binds_all`` and the ECMAScript rows carry
#   the names their import binds. Reusing v7 rows would read every C
#   ``#include`` and every ``from x import *`` back as an import that binds no
#   name, and every TypeScript named import back as one that binds nothing --
#   which is precisely the whole-module over-promotion this version exists to
#   end, reintroduced by a warm cache.
# 9 (2026-08-23): import rows carry the local name an import binds -- ``alias``
#   for a module object, ``local_names`` for the members of a ``from`` import
#   -- and reference rows carry the qualifier as the name the source spells
#   rather than the module behind it. Reusing v8 rows would key every module
#   binding on a name the importing file does not bind, so `import a.b as ab`
#   would resolve `ab.f()` through `a` and attribute it to the package.
# 10 (2026-08-23): the ``tags.qualname``, ``tags.is_def``, ``files.size`` and
#   ``files.lang`` columns are gone. Every one of them was written on every row
#   and selected by nothing. A v9 database still declares them ``NOT NULL``, so
#   this build's shorter INSERT would fail on the first file of every index run
#   and leave the repository with no usable cache rather than with a smaller
#   one. The bump drops the tables instead.
#
#   The same bump covers the extractor changes that landed beside it, because
#   row reuse keys on the content digest and the grammar version, so an
#   unchanged file would otherwise serve rows this build no longer agrees
#   with. Reference rows carry a new ``builtin`` role, and a walrus target and
#   a class-body comprehension name changed role; tag rows carry a keyword the
#   source actually writes in the signature (``interface``, ``type``,
#   ``struct``) and an owner on a nested class, which changes its stable id. A
#   v9 reference row read by this build is fine; a ``builtin`` row read by an
#   OLDER build raises at ``IdentifierRole(...)``, which is the other half of
#   why one bump has to cover both.
SCHEMA_VERSION = 10

ENV_CACHE_HOME = "XDG_CACHE_HOME"
ENV_NO_AUTO_INDEX = "AGENTLESS_MCP_NO_AUTO_INDEX"
APPLICATION_DIR = "agentless-mcp"
DATABASE_NAME = "tags.db"
LOCK_NAME = "write.lock"

logger = logging.getLogger(__name__)

# 64 bits of realpath digest: enough that two repositories on one machine do
# not collide, short enough that the directory name is still readable. The
# repository's own path is stored in ``meta`` and checked on open, so a
# collision degrades to "cache: none" rather than to a wrong answer.
KEY_LENGTH = 16

NOGIT_PREFIX = "nogit:"

# How much of the manifest digest the non-git generation stamp keeps. Its own
# constant rather than ``KEY_LENGTH``: the two happen to be the same width and
# answer unrelated questions, so retuning the directory key must not re-spell
# every generation stamp with it.
MANIFEST_DIGEST_LENGTH = 16

# SQLite is out-of-process state like any other: a lock wait gets a bound.
SQLITE_TIMEOUT_SECONDS = 5.0

# Directories under the user's cache home hold derived facts about private
# repositories, so they are owner-only.
DIRECTORY_MODE = 0o700

RECEIPT_NONE = "none"
RECEIPT_BYPASSED = "bypassed (--no-cache)"
REMEDIATION = MESSAGES.cache_stale_remediation
STALE_REFRESHING = MESSAGES.cache_stale_refreshing
ABSENT_REFRESHING = MESSAGES.cache_absent_refreshing

# What one file's extraction may fail with without taking the scan down with
# it. A repository index is a per-file job: a grammar that will not load, a
# generated file whose expression nests deeper than the interpreter's stack,
# a byte sequence no decoder accepts -- each is a fact about that one file and
# belongs in ``IndexFailure``, not in a traceback out of ``agentless-mcp
# index``. Named explicitly rather than catching ``Exception``: an error class
# not in this list is a defect in the extractor and must surface as one.
# ``core.patchlint`` keeps its own such list, ``DEGRADED_ERRORS``, for the same
# reason on the same parse path; the two are separate rather than shared
# because ``patchlint`` sits above the cache in the module graph and importing
# it here would invert that. ``UnicodeDecodeError`` is absent from both because
# it is a ``ValueError`` and is caught by that member.
EXTRACTION_FAILURES: tuple[type[Exception], ...] = (
    AgentlessError,
    ValueError,
    RecursionError,
    OSError,
)


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
    generation_matches: bool
    enabled: bool
    files: int
    tags: int
    note: str

    @property
    def receipt(self) -> str:
        """Return the ``cache:`` field of the response receipt.

        While a background refresh is running for this database the
        remediation names it instead of telling the agent to reindex --
        advice that would race the refresh already doing exactly that.
        """
        if not self.enabled:
            return RECEIPT_BYPASSED
        refreshing = auto_index_in_progress(self.path)
        if self.generation is None:
            notes = "; ".join(
                note for note in (self.note, ABSENT_REFRESHING if refreshing else "") if note
            )
            return f"{RECEIPT_NONE} ({notes})" if notes else RECEIPT_NONE
        if self.generation_matches:
            return f"g:{self.generation} fresh"
        return (
            f"g:{self.generation} generation mismatch (repo g:{self.repo_generation}); "
            f"{STALE_REFRESHING if refreshing else REMEDIATION}"
        )

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this status."""
        return {
            "receipt": self.receipt,
            "path": str(self.path) if self.path is not None else None,
            "generation": self.generation,
            "repo_generation": self.repo_generation,
            "generation_matches": self.generation_matches,
            "enabled": self.enabled,
            "files": self.files,
            "tags": self.tags,
            "note": self.note,
        }


class FileSource(Protocol):
    """Where a view gets one file's parsed facts from.

    The seam that keeps the cache invisible above :mod:`agentless_mcp.core`:
    a service asks for the symbols, imports or references of a text it already
    holds and never learns whether they were parsed or read back from SQLite.
    """

    @property
    def receipt(self) -> str:
        """The ``cache:`` field describing this source."""
        ...

    def symbols_for(self, text: str, language: str, path: str) -> list[ASTSymbol]:
        """Return the symbols ``text`` defines, as the extractor would."""
        ...

    def imports_for(self, text: str, language: str, path: str) -> list[ImportStatement]:
        """Return the imports ``text`` declares, as the extractor would."""
        ...

    def refs_for(self, text: str, language: str, path: str) -> list[Ref]:
        """Return the identifier occurrences in ``text``, as the extractor would."""
        ...

    def status(self) -> CacheStatus:
        """Describe this source, including row counts when it has any."""
        ...

    def close(self) -> None:
        """Release resources held by this source."""
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

    def imports_for(self, text: str, language: str, path: str) -> list[ImportStatement]:
        """Extract imports from ``text`` with no cache involved."""
        return self._extractor.extract_imports_from_source(text, language, path)

    def refs_for(self, text: str, language: str, path: str) -> list[Ref]:
        """Extract identifier references from ``text`` with no cache involved."""
        return self._extractor.extract_refs_from_source(text, language, path)

    def status(self) -> CacheStatus:
        """Describe why there is no cache behind this source."""
        return self._status

    def close(self) -> None:
        """Release resources; on-demand parsing owns none."""


@dataclass(frozen=True)
class _RowCounts:
    """How many rows of each kind one database holds."""

    files: int
    tags: int
    imports: int
    refs: int


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
        rows = self._rows_or_none(self._symbol_rows, text, path, "symbol")
        if rows is None:
            return self._extractor.extract_from_source(text, language, path)
        return rows

    def imports_for(self, text: str, language: str, path: str) -> list[ImportStatement]:
        """Return cached imports when the row still describes ``text``."""
        rows = self._rows_or_none(self._import_rows, text, path, "import")
        if rows is None:
            return self._extractor.extract_imports_from_source(text, language, path)
        return rows

    def refs_for(self, text: str, language: str, path: str) -> list[Ref]:
        """Return cached identifier references when the row still describes ``text``."""
        rows = self._rows_or_none(self._ref_rows, text, path, "reference")
        if rows is None:
            return self._extractor.extract_refs_from_source(text, language, path)
        return rows

    def _rows_or_none(
        self,
        read: "Callable[[str, str], list[Any]]",
        text: str,
        path: str,
        kind: str,
    ) -> list[Any] | None:
        """Read one file's rows, or return ``None`` to mean "parse it instead".

        The module promises that a read command never fails because of a
        cache, and until now the three row readers had no handler at all: a
        corrupt page, a truncated row, or an enum value written by a build
        that spelled it differently raised straight out of a tool call. All
        three are the same event -- persisted bytes this build cannot read --
        and the answer to all three is the answer to a stale digest, which is
        to parse the file.

        The exception list is long because it enumerates the ways a row can
        be wrong rather than catching everything: sqlite for the file itself,
        ValueError for an unknown ``SymbolKind`` or ``IdentifierRole`` and for
        malformed JSON, TypeError for rationale JSON of the wrong shape,
        IndexError and KeyError for a row or document missing a field this
        build reads. A defect in this module still raises.
        """
        digest = self._fresh_digest(text, path)
        if digest is None:
            return None
        try:
            return read(path, digest)
        except (sqlite3.DatabaseError, ValueError, TypeError, IndexError, KeyError) as exc:
            logger.warning(
                "tag cache %s: %s rows for %s are unreadable (%r); parsing the file instead",
                self._state.database,
                kind,
                path,
                exc,
            )
            return None

    def status(self) -> CacheStatus:
        """Describe the cache, counting its rows."""
        counts = _row_counts(self._connection)
        return CacheStatus(
            path=self._state.database,
            generation=self._state.generation,
            repo_generation=self._state.repo_generation,
            generation_matches=self._state.generation == self._state.repo_generation,
            enabled=True,
            files=counts.files,
            tags=counts.tags,
            note="",
        )

    def close(self) -> None:
        """End the read snapshot, then release the connection.

        Ended explicitly rather than left to ``close`` so the reason is
        visible here: the open transaction is what holds the WAL from being
        checkpointed, and a source that is never closed would grow it for as
        long as the process lives.
        """
        try:
            self._connection.execute("ROLLBACK")
        except sqlite3.DatabaseError as exc:
            logger.debug("read snapshot for %s was already gone: %r", self._state.database, exc)
        self._connection.close()

    def _fresh_digest(self, text: str, path: str) -> str | None:
        """Return the digest to read rows at, or None when they cannot be used.

        One gate for all three row kinds: the file must be indexed, indexed by
        the grammar pack that is installed now, and indexed from exactly this
        content. Anything else and the caller parses.
        """
        entry = self._state.entries.get(path)
        if entry is None or entry.grammar_version != self._state.grammar_version:
            return None
        digest = content_digest(text)
        return digest if entry.digest == digest else None

    def _symbol_rows(self, path: str, digest: str) -> list[ASTSymbol]:
        """Rebuild one file's symbols from its tag rows, in extraction order.

        The collision ordinal is recomputed rather than stored: it is a pure
        function of one file's symbol list, the rows come back in the order
        the extractor produced them, and deriving it here is what guarantees a
        cached answer and an on-demand one carry the same stable ids.
        """
        cursor = self._connection.execute(
            "SELECT name, kind, start_line, end_line, signature, parent, docstring, "
            "decorators, bases, language, is_public, is_async, rationales "
            "FROM tags WHERE path = ? AND sha256 = ? ORDER BY ordinal",
            (path, digest),
        )
        return disambiguate([_symbol_from_row(row, path) for row in cursor.fetchall()])

    def _import_rows(self, path: str, digest: str) -> list[ImportStatement]:
        """Rebuild one file's import statements from its rows, in extraction order."""
        cursor = self._connection.execute(
            "SELECT module, names, is_relative, relative_level, line, resolved_path, "
            "binds_all, alias, local_names FROM imports "
            "WHERE path = ? AND sha256 = ? ORDER BY ordinal",
            (path, digest),
        )
        return [_import_from_row(row) for row in cursor.fetchall()]

    def _ref_rows(self, path: str, digest: str) -> list[Ref]:
        """Rebuild one file's identifier references from its rows, in order."""
        cursor = self._connection.execute(
            "SELECT name, line, role, qualifier FROM refs "
            "WHERE path = ? AND sha256 = ? ORDER BY ordinal",
            (path, digest),
        )
        return [
            Ref(
                path=path,
                name=str(row[0]),
                line=int(row[1]),
                role=IdentifierRole(str(row[2])),
                qualifier=str(row[3]),
            )
            for row in cursor.fetchall()
        ]


def effective_source(
    source: FileSource | None,
    extractor: TreeSitterExtractor,
) -> FileSource:
    """Return ``source``, or an on-demand source when a call carries none.

    Callers that never opened a cache -- the test suite, an embedding library
    user -- get the index-free path without having to construct anything.
    """
    return source if source is not None else OnDemandSource(extractor)


def cache_root() -> Path:
    """Return the directory holding every repository's cache, per XDG.

    A relative ``XDG_CACHE_HOME`` is ignored, which is what the XDG base
    directory specification requires -- "if an implementation encounters a
    relative path it must consider the value invalid" -- and which this
    module needs for a reason of its own. A relative value resolves against
    the current working directory, and the working directory during a
    ``validate`` run is the repository being analysed. Reproduced:
    ``cd victim && XDG_CACHE_HOME=relcache agentless-mcp validate --repo
    victim`` created ``victim/relcache/agentless-mcp/worktrees`` inside the
    repository under analysis. It also made the cache location depend on
    where each call happened to be standing, so two calls in one process
    could read two different databases.
    """
    configured = os.environ.get(ENV_CACHE_HOME, "").strip()
    if configured and not Path(configured).is_absolute():
        _warn_once_about_relative_cache_home(configured)
        configured = ""
    home = Path(configured) if configured else Path.home() / ".cache"
    return home / APPLICATION_DIR


_RELATIVE_CACHE_HOMES_SEEN: set[str] = set()


def _warn_once_about_relative_cache_home(value: str) -> None:
    """Say why the environment was ignored, once per distinct value.

    Once rather than per call: ``cache_root`` runs on every cached read, and a
    warning per read would bury the answer it is attached to.
    """
    if value in _RELATIVE_CACHE_HOMES_SEEN:
        return
    _RELATIVE_CACHE_HOMES_SEEN.add(value)
    logger.warning(
        "%s=%r is relative and was ignored; the XDG specification requires an absolute "
        "path, and a relative one would put the cache inside whichever directory the "
        "call was made from -- including the repository being analysed",
        ENV_CACHE_HOME,
        value,
    )


def cache_path(repo_root: Path) -> Path:
    """Return the database path for one repository."""
    key = hashlib.sha256(str(repo_root.resolve()).encode("utf-8")).hexdigest()[:KEY_LENGTH]
    return cache_root() / key / DATABASE_NAME


def _ensure_cache_directory(directory: Path) -> None:
    """Create one repository's cache directory, owner-only at both levels.

    ``mkdir(mode=...)`` sets the mode of the leaf and of nothing else, so a
    ``parents=True`` call left ``<cache home>/agentless-mcp`` at whatever the
    umask allowed, and ``exist_ok=True`` never re-applied the mode to a
    directory an earlier version had already created. Both are chmodded after
    the fact so :data:`DIRECTORY_MODE` describes an upgraded install and not
    only a fresh one. The database file itself stays at SQLite's own mode:
    an owner-only directory is what makes it unreachable.

    A filesystem that will not carry the mode is reported and not fatal --
    indexing a repository is still the useful thing to do on it.
    """
    directory.mkdir(parents=True, exist_ok=True, mode=DIRECTORY_MODE)
    for path in (directory.parent, directory):
        try:
            path.chmod(DIRECTORY_MODE)
        except OSError as exc:
            logger.warning("could not restrict %s to owner-only: %r", path, exc)


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
) -> FileSource:
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
    return _open_indexed(database, resolved, extractor, tree_oid=tree_oid)


def _open_indexed(
    database: Path,
    repo_root: Path,
    extractor: TreeSitterExtractor,
    *,
    tree_oid: str | None,
) -> FileSource:
    """Open an existing database, or say in the receipt why it is not consulted."""
    # Two ways to fail, two answers. ``sqlite3.DatabaseError`` proper is the
    # unusable file -- "file is not a database", a malformed disk image -- and
    # that file is derived data that failed, so it goes. ``OperationalError``
    # is the environment around the file: a disk I/O error, a database that
    # will not open, a lock wait that timed out. None of those says anything
    # about what the index holds, so this call parses live and leaves the file
    # for the next one. Unlinking on them deleted healthy indexes.
    try:
        connection = _connect(database)
    except sqlite3.OperationalError as exc:
        return OnDemandSource(extractor, _absent_status(database, f"cache unreadable: {exc}"))
    except sqlite3.DatabaseError as exc:
        _discard_unlocked(database, repo_root)
        return OnDemandSource(extractor, _absent_status(database, f"cache discarded: {exc}"))

    try:
        # One snapshot for every read this source will serve. The freshness
        # gate consults `entries`, captured here; the row reads that follow
        # are separate statements, and in autocommit each of those saw the
        # database as it was at that instant. The background index thread
        # this very request started can delete the rows between the two, and
        # `_symbol_rows` then returned an empty list as fact -- a file with
        # no symbols, rather than a cache miss. A deferred read transaction
        # pins both to the same view. WAL is what makes this free: readers do
        # not block the writer, so the index keeps running underneath.
        connection.execute("BEGIN")
        opened = _read_index(connection, repo_root)
    except sqlite3.OperationalError as exc:
        connection.close()
        return OnDemandSource(extractor, _absent_status(database, f"cache unreadable: {exc}"))
    except sqlite3.DatabaseError as exc:
        connection.close()
        _discard_unlocked(database, repo_root)
        return OnDemandSource(extractor, _absent_status(database, f"cache discarded: {exc}"))

    meta = opened.meta
    if meta is None:
        connection.close()
        _discard_unlocked(database, repo_root)
        return OnDemandSource(extractor, _absent_status(database, opened.note))

    return CachedSource(
        connection,
        extractor,
        _CacheState(
            database=database,
            entries=opened.entries,
            generation=meta.generation,
            repo_generation=repo_generation(repo_root, tree_oid),
            grammar_version=grammars.pack_version(),
        ),
    )


@dataclass(frozen=True)
class IndexFailure:
    """One file the index run could not record, and why."""

    path: str
    reason: str


# The grammar-version stamp recorded for a file whose language is known but
# whose grammar is not warmed. It can never equal an installed pack version,
# so the row is reused for as long as the language stays unwarmed and misses
# every digest gate -- the next index run after a warmup re-extracts the file,
# and a read in the meantime parses on demand instead of trusting empty rows.
UNWARMED_STAMP_PREFIX = "unwarmed:"


@dataclass(frozen=True)
class IndexSkip:
    """One file the run declined to extract facts from, and why.

    Two reasons reach it: the file's grammar is not warmed, or the file is
    over the per-file read cap. Both are decisions, so both are downgraded
    from the error class on purpose. A fresh install has only the tier-1
    grammars warmed, and a repository's own config files must not fail
    ``index`` over a grammar the operator was never asked to fetch; an
    oversized file was never going to be read at all. An unwarmed file is
    recorded with its digest so the next run reuses the row instead of
    re-attempting it. An oversized one has no digest to record.
    """

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
    imports: int
    refs: int
    failures: tuple[IndexFailure, ...]
    skipped_files: tuple[IndexSkip, ...]

    @property
    def errors(self) -> int:
        """How many files could not be recorded."""
        return len(self.failures)

    @property
    def skipped(self) -> int:
        """How many known-language files were recorded without facts."""
        return len(self.skipped_files)

    def summary_line(self) -> str:
        """Return the one-line summary the CLI prints."""
        return (
            f"indexed {self.indexed}, reused {self.reused}, pruned {self.pruned}, "
            f"skipped {self.skipped}, errors {self.errors}: {self.files} files, "
            f"{self.tags} tags, {self.imports} imports, {self.refs} refs "
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
            "imports": self.imports,
            "refs": self.refs,
            "skipped": self.skipped,
            "failures": [{"path": entry.path, "reason": entry.reason} for entry in self.failures],
            "skipped_files": [
                {"path": entry.path, "reason": entry.reason} for entry in self.skipped_files
            ],
        }


@dataclass(frozen=True)
class _FileTags:
    """One file's recorded state: its digest and everything parsed out of it.

    ``grammar_version`` is per file rather than per run because an unwarmed
    file is stamped :data:`UNWARMED_STAMP_PREFIX` plus the pack version, which
    is what makes its empty row expire the moment the grammar is warmed.
    """

    path: str
    digest: str
    symbols: tuple[ASTSymbol, ...]
    imports: tuple[ImportStatement, ...]
    refs: tuple[Ref, ...]
    grammar_version: str

    @property
    def unwarmed(self) -> bool:
        """Whether this row records a skip rather than extracted facts."""
        return self.grammar_version.startswith(UNWARMED_STAMP_PREFIX)


@dataclass(frozen=True)
class _IndexPlan:
    """The whole index run decided before a single row is written."""

    writes: tuple[_FileTags, ...]
    reused: tuple[str, ...]
    seen: frozenset[str]
    present: frozenset[str]
    failures: tuple[IndexFailure, ...]
    skips: tuple[IndexSkip, ...]


def auto_index_disabled() -> bool:
    """True when the environment opts out of the background index refresh."""
    return os.environ.get(ENV_NO_AUTO_INDEX, "") not in ("", "0", "false", "False")


# The generation recorded for an attempt that never got far enough to read
# one. It cannot equal a git tree oid or a ``nogit:`` stamp, so the record
# reads as "this process already tried and stopped" rather than as a
# generation the index is at.
GENERATION_UNAVAILABLE = "unavailable"


@dataclass
class _AutoIndexRun:
    """One repository's background refresh: its thread and the generation it targets.

    The record outlives the thread so a completed run is not repeated for the
    same generation -- success or failure, one attempt per generation per
    process, exactly the auto-warm policy. A new commit changes the
    generation and re-arms the trigger.

    ``thread`` is None for an attempt that stopped before it started one. The
    record is still what stops the attempt from being repeated.
    """

    thread: threading.Thread | None
    generation: str


# Keyed by database path rather than repository root because the database is
# what the refresh writes. Guarded by a mutex because the HTTP transport
# resolves repositories from concurrent request threads.
_AUTO_INDEX_LOCK = threading.Lock()
_AUTO_INDEX_RUNS: dict[Path, _AutoIndexRun] = {}


def auto_index_in_progress(database: Path | None) -> bool:
    """True while a background refresh of ``database`` is still running."""
    if database is None:
        return False
    with _AUTO_INDEX_LOCK:
        run = _AUTO_INDEX_RUNS.get(database)
    return run is not None and run.thread is not None and run.thread.is_alive()


def _record_attempt(database: Path, generation: str) -> None:
    """Record that this process tried to refresh ``database`` and stopped.

    No thread, because none started. A live run is never overwritten: a
    caller that failed while another one was already indexing must not erase
    the record of the run that is doing the work.
    """
    with _AUTO_INDEX_LOCK:
        current = _AUTO_INDEX_RUNS.get(database)
        if current is not None and current.thread is not None and current.thread.is_alive():
            return
        _AUTO_INDEX_RUNS[database] = _AutoIndexRun(thread=None, generation=generation)


def start_auto_index(
    root: Path,
    extractor: TreeSitterExtractor,
    *,
    tree_oid: str | None = None,
    head_sha: str | None = None,
) -> threading.Thread | None:
    """Start one background refresh of a stale tag cache; never blocks or raises.

    Per repository on first use rather than for every configured root at
    startup: a server can hold many roots, and walking repositories no call
    ever asks about is work nobody ordered. The MCP server is the only
    caller on purpose -- a one-shot CLI process would kill the daemon thread
    at exit before its single end-of-run transaction commits, starting over
    every invocation and finishing never; the CLI's path is the explicit
    ``index`` command.

    Returns the running thread, or ``None`` when there is nothing to do:
    the environment opts out, the index already describes the repository's
    generation, or this process already made its attempt at that generation.
    """
    if auto_index_disabled():
        return None
    resolved = root.resolve()
    database = cache_path(resolved)

    with _AUTO_INDEX_LOCK:
        run = _AUTO_INDEX_RUNS.get(database)
    if run is not None and run.thread is not None and run.thread.is_alive():
        return run.thread
    if run is not None and run.generation == GENERATION_UNAVAILABLE:
        # This process already failed to read what generation this repository
        # is at. Nothing about the repository can re-arm a trigger whose stamp
        # cannot be read, and the attempt is the expensive half, so it is not
        # made again until the process restarts.
        return None

    # Outside the git tree oid this walks the repository's stat manifest.
    generation = GENERATION_UNAVAILABLE
    try:
        generation = repo_generation(resolved, tree_oid)
        done = (run is not None and run.generation == generation) or _index_current(
            database, resolved, generation
        )
    except (AgentlessError, OSError) as error:
        # Registered before returning, exactly as a failure inside the thread
        # is. Without the record this walked the repository again and logged
        # this line again on every MCP call -- the per-call retry storm the
        # one-attempt-per-generation rule exists to prevent, on the one path
        # that was not covered by it.
        _record_attempt(database, generation)
        logger.warning("background index refresh for %s skipped: %s", resolved, error)
        return None
    if done:
        return None

    return _start_refresh(resolved, extractor, generation, tree_oid, head_sha)


def _start_refresh(
    root: Path,
    extractor: TreeSitterExtractor,
    generation: str,
    tree_oid: str | None,
    head_sha: str | None,
) -> threading.Thread | None:
    """Register and start this generation's refresh, unless a racer got there first.

    Any record already holding this generation was written by a racing caller:
    the record ``start_auto_index`` read before it computed the generation was
    tested against it there, and a match returns before this runs.
    """
    database = cache_path(root)
    with _AUTO_INDEX_LOCK:
        raced = _AUTO_INDEX_RUNS.get(database)
        if raced is not None and raced.generation == generation:
            return raced.thread
        thread = threading.Thread(
            target=_auto_index,
            args=(root, extractor, tree_oid, head_sha),
            name="tag-auto-index",
            daemon=True,
        )
        _AUTO_INDEX_RUNS[database] = _AutoIndexRun(thread=thread, generation=generation)
        # Started under the lock, not after it. A thread that is registered but
        # not yet started reads as ``is_alive() == False``, so a second caller
        # that had already found the registry empty saw no live run, fell past
        # the guard, and started a duplicate index of the same generation. The
        # guard now keys on the generation this run targets -- the thing the
        # one-attempt-per-generation rule is actually about -- rather than on
        # whether the thread has reached its first instruction.
        thread.start()
    return thread


def _auto_index(
    root: Path,
    extractor: TreeSitterExtractor,
    tree_oid: str | None,
    head_sha: str | None,
) -> None:
    """Refresh one repository's tag cache; one log line, never an exception out."""
    # Index warm grammars rather than racing the startup warm: an index built
    # over cold grammars records unwarmed stamps that expire on warmup anyway,
    # so waiting the warm's own bounded deadline buys a one-pass index.
    grammars.wait_for_auto_warm()
    started = time.monotonic()
    try:
        report = build_index(root, extractor, tree_oid=tree_oid, head_sha=head_sha)
    except CacheLocked:
        # Another index run is refreshing the same database. Another index
        # run rather than another process: ``flock`` is per open file
        # description, so a second thread of this process is refused the same
        # way and reads the same line. Its result serves this one too; a queue
        # here would re-run work already done.
        logger.info(
            "background index refresh for %s skipped: another index run holds the index lock",
            root,
        )
        return
    except (AgentlessError, sqlite3.DatabaseError, OSError) as error:
        # The contract is one log line and today's parse-live behavior,
        # never a crashed thread mid-session.
        logger.warning("background index refresh for %s failed: %s", root, error)
        return
    logger.info(
        "background index refresh in %.1fs: %s",
        time.monotonic() - started,
        report.summary_line(),
    )


def _index_current(database: Path, repo_root: Path, generation: str) -> bool:
    """Whether the index already describes ``generation``.

    A missing, unreadable or rejected database is not current -- each is
    exactly the state a refresh exists to replace, and ``build_index``
    handles all of them.
    """
    if not database.exists():
        return False
    try:
        connection = _connect(database)
    except sqlite3.DatabaseError:
        return False
    try:
        meta = _read_meta(connection)
    except sqlite3.DatabaseError:
        return False
    finally:
        connection.close()
    return meta is not None and not _rejection(meta, repo_root) and meta.generation == generation


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
    precisely so they are skipped next time. A known language whose grammar is
    not warmed is recorded the same way -- digest kept, facts absent -- and
    reported in ``skipped_files`` rather than in ``failures``.
    """
    resolved = root.resolve()
    database = cache_path(resolved)
    _ensure_cache_directory(database.parent)

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
            counts = _row_counts(connection)
        finally:
            connection.close()

    # Counted from the paths that left the repository, not from the
    # complement of ``plan.seen``. A file whose extraction failed is dropped
    # from ``seen`` so its rows go, and an unreadable one never enters it, but
    # both are still in the tree: counting them here put one file in both
    # ``errors`` and ``pruned`` on the same summary line and made ``pruned``
    # stop meaning "files that left the repository".
    pruned = len([path for path in previous if path not in plan.present])
    return IndexReport(
        database=database,
        generation=generation,
        indexed=sum(1 for entry in plan.writes if not entry.unwarmed),
        reused=len(plan.reused),
        pruned=pruned,
        files=counts.files,
        tags=counts.tags,
        imports=counts.imports,
        refs=counts.refs,
        failures=plan.failures,
        skipped_files=plan.skips,
    )


def _plan_index(
    root: Path,
    extractor: TreeSitterExtractor,
    *,
    previous: dict[str, _FileEntry],
    force: bool,
) -> _IndexPlan:
    """Walk the repository and decide, per file, reuse, skip or re-extract."""
    grammar_version = grammars.pack_version()
    unwarmed_version = UNWARMED_STAMP_PREFIX + grammar_version
    warmed = grammars.warmed_languages()
    writes: list[_FileTags] = []
    reused: list[str] = []
    seen: set[str] = set()
    present: set[str] = set()
    failures: list[IndexFailure] = []
    skips: list[IndexSkip] = []

    for repo_file in walk_repo(root):
        language = TreeSitterExtractor.SUPPORTED_EXTENSIONS.get(Path(repo_file.path).suffix)
        if language is None:
            continue

        present.add(repo_file.path)

        if repo_file.size > DEFAULT_MAX_FILE_BYTES:
            # Declining to read a file is a skip, not a failure to read one.
            # Decided from the walk's own stat rather than from the wording of
            # ``read_bounded``'s skip reason: the invariant is "this file is
            # over the cap", and a message is not a category. Deciding it
            # first also means the bytes are never pulled in to be dropped.
            skips.append(
                IndexSkip(
                    path=repo_file.path,
                    reason=(
                        f"skipped: {repo_file.size} bytes exceeds the per-file "
                        f"cap of {DEFAULT_MAX_FILE_BYTES} bytes"
                    ),
                )
            )
            continue

        # The same cap the stat above was compared against, passed rather than
        # defaulted: the two deciding the same number is what keeps a
        # cap-exceeded read out of ``failures``.
        read = read_bounded(root / repo_file.path, DEFAULT_MAX_FILE_BYTES)
        if read.text is None:
            failures.append(IndexFailure(path=repo_file.path, reason=read.skipped or "unreadable"))
            continue

        seen.add(repo_file.path)
        digest = content_digest(read.text)
        entry = previous.get(repo_file.path)

        if language not in warmed:
            # A skip, not a failure: the same wording get_language raises with,
            # so the index report and a live scan describe the file identically.
            skips.append(
                IndexSkip(path=repo_file.path, reason=grammars.unavailable_reason(language))
            )
            if not force and entry == _FileEntry(digest, unwarmed_version):
                continue
            writes.append(
                _FileTags(
                    path=repo_file.path,
                    digest=digest,
                    symbols=(),
                    imports=(),
                    refs=(),
                    grammar_version=unwarmed_version,
                )
            )
            continue

        if not force and entry is not None and entry == _FileEntry(digest, grammar_version):
            reused.append(repo_file.path)
            continue

        try:
            symbols = extractor.extract_from_source(read.text, language, repo_file.path)
            imports = extractor.extract_imports_from_source(read.text, language, repo_file.path)
            refs = extractor.extract_refs_from_source(read.text, language, repo_file.path)
        except EXTRACTION_FAILURES as exc:
            # The class name is part of the report: "maximum recursion depth
            # exceeded" names a defect, a bare KeyError message names nothing.
            reason = f"{type(exc).__name__}: {exc}"
            seen.discard(repo_file.path)
            failures.append(IndexFailure(path=repo_file.path, reason=reason))
            continue

        writes.append(
            _FileTags(
                path=repo_file.path,
                digest=digest,
                symbols=tuple(symbols),
                imports=tuple(imports),
                refs=tuple(refs),
                grammar_version=grammar_version,
            )
        )

    return _IndexPlan(
        writes=tuple(writes),
        reused=tuple(reused),
        seen=frozenset(seen),
        present=frozenset(present),
        failures=tuple(failures),
        skips=tuple(skips),
    )


def _apply_plan(
    connection: sqlite3.Connection,
    plan: _IndexPlan,
    meta: _Meta,
    previous: dict[str, _FileEntry],
) -> None:
    """Write the whole plan in one transaction, or none of it."""
    vanished = [path for path in previous if path not in plan.seen]
    touched = [entry.path for entry in plan.writes]

    connection.execute("BEGIN IMMEDIATE")
    try:
        for path in (*vanished, *touched):
            # Spelled out rather than looped over a table-name list: a query
            # built by interpolating a name is the shape of an injection even
            # when today's names are constants.
            connection.execute("DELETE FROM tags WHERE path = ?", (path,))
            connection.execute("DELETE FROM imports WHERE path = ?", (path,))
            connection.execute("DELETE FROM refs WHERE path = ?", (path,))
            connection.execute("DELETE FROM files WHERE path = ?", (path,))

        for entry in plan.writes:
            connection.execute(
                "INSERT INTO files (path, sha256, grammar_version) VALUES (?, ?, ?)",
                (entry.path, entry.digest, entry.grammar_version),
            )
            connection.executemany(
                "INSERT INTO tags (path, sha256, name, kind, start_line, end_line, "
                "signature, parent, docstring, decorators, bases, language, "
                "is_public, is_async, rationales, ordinal) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    _tag_row(entry.path, entry.digest, ordinal, symbol)
                    for ordinal, symbol in enumerate(entry.symbols)
                ],
            )
            connection.executemany(
                "INSERT INTO imports (path, sha256, module, names, is_relative, "
                "relative_level, line, resolved_path, binds_all, alias, local_names, "
                "ordinal) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    _import_row(entry.path, entry.digest, ordinal, statement)
                    for ordinal, statement in enumerate(entry.imports)
                ],
            )
            connection.executemany(
                "INSERT INTO refs (path, sha256, name, line, role, qualifier, ordinal) "
                "VALUES (?, ?, ?, ?, ?, ?, ?)",
                [
                    (
                        entry.path,
                        entry.digest,
                        ref.name,
                        ref.line,
                        ref.role.value,
                        ref.qualifier,
                        ordinal,
                    )
                    for ordinal, ref in enumerate(entry.refs)
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
    """Flatten one symbol into its tag row."""
    return (
        path,
        digest,
        symbol.name,
        symbol.kind.value,
        symbol.line_number,
        symbol.end_line_number,
        symbol.signature,
        symbol.parent_class,
        symbol.docstring,
        json.dumps(list(symbol.decorators)),
        json.dumps(list(symbol.bases)),
        symbol.language,
        int(symbol.is_public),
        int(symbol.is_async),
        json.dumps(
            [
                {
                    "kind": rationale.kind,
                    "text": rationale.text,
                    "line": rationale.line_number,
                    "citations": list(rationale.citations),
                    "duplicate_index": rationale.duplicate_index,
                }
                for rationale in symbol.rationales
            ]
        ),
        ordinal,
    )


def _import_row(
    path: str, digest: str, ordinal: int, statement: ImportStatement
) -> tuple[Any, ...]:
    """Flatten one import statement into its row."""
    return (
        path,
        digest,
        statement.module,
        json.dumps(list(statement.names)),
        int(statement.is_relative),
        statement.relative_level,
        statement.line_number,
        statement.resolved_path,
        int(statement.binds_all),
        statement.alias,
        json.dumps(list(statement.local_names)),
        ordinal,
    )


def _import_from_row(row: Sequence[Any]) -> ImportStatement:
    """Rebuild one import statement from its row, converting explicitly."""
    return ImportStatement(
        module=str(row[0]),
        names=tuple(str(item) for item in json.loads(str(row[1]))),
        is_relative=bool(row[2]),
        relative_level=int(row[3]),
        line_number=int(row[4]),
        resolved_path=str(row[5]),
        binds_all=bool(row[6]),
        alias=str(row[7]),
        local_names=tuple(str(item) for item in json.loads(str(row[8]))),
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
        rationales=_rationales_from_row(row[12]),
    )


def _rationales_from_row(raw: Any) -> tuple[Rationale, ...]:
    """Parse cached rationale JSON into bounded domain values."""
    document = json.loads(str(raw))
    if not isinstance(document, list):
        message = "cached rationales must be a JSON array"
        raise TypeError(message)

    rationales: list[Rationale] = []
    for item in document:
        if not isinstance(item, dict):
            message = "each cached rationale must be a JSON object"
            raise TypeError(message)
        citations = item["citations"]
        if not isinstance(citations, list):
            message = "cached rationale citations must be a JSON array"
            raise TypeError(message)
        rationales.append(
            Rationale(
                kind=str(item["kind"]),
                text=str(item["text"]),
                line_number=int(item["line"]),
                citations=tuple(str(citation) for citation in citations),
                duplicate_index=int(item["duplicate_index"]),
            )
        )
    return tuple(rationales)


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
    try:
        connection.execute("PRAGMA journal_mode=WAL")
        connection.execute("PRAGMA synchronous=NORMAL")
    except BaseException:
        connection.close()
        raise
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
        connection.executescript(_DROP_SCHEMA)

    connection.executescript(_SCHEMA)


# A version bump drops every table this package owns and rebuilds them. The
# list is exhaustive on purpose: a table left behind by an older schema would
# be read by the new code as if the new code had written it.
_DROP_SCHEMA = """
DROP TABLE IF EXISTS tags;
DROP TABLE IF EXISTS imports;
DROP TABLE IF EXISTS refs;
DROP TABLE IF EXISTS files;
DROP TABLE IF EXISTS meta;
"""

_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    schema_version INTEGER NOT NULL,
    repo_root TEXT NOT NULL,
    generation_tree_oid TEXT NOT NULL,
    head_sha TEXT,
    created_at TEXT NOT NULL
);
-- Every column here is read back. A column the indexer fills on every file
-- and no reader ever selects is storage and write time spent on nothing, and
-- the migration policy -- bump and rebuild -- makes adding one back free on
-- the day a reader wants it.
CREATE TABLE IF NOT EXISTS files (
    path TEXT PRIMARY KEY,
    sha256 TEXT NOT NULL,
    grammar_version TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tags (
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
-- References are by far the largest table (tens of thousands of rows per
-- repository against hundreds of symbols), and every read of them is
-- "one file, one digest, in order". A WITHOUT ROWID table keyed on exactly
-- that clusters the rows the way they are read and removes the separate
-- (path, sha256) index, which measured the same size as the table it indexed.
CREATE TABLE IF NOT EXISTS imports (
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    module TEXT NOT NULL,
    names TEXT NOT NULL,
    is_relative INTEGER NOT NULL,
    relative_level INTEGER NOT NULL,
    line INTEGER NOT NULL,
    resolved_path TEXT NOT NULL,
    binds_all INTEGER NOT NULL,
    alias TEXT NOT NULL,
    local_names TEXT NOT NULL,
    ordinal INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS refs (
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    name TEXT NOT NULL,
    line INTEGER NOT NULL,
    role TEXT NOT NULL,
    qualifier TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (path, sha256, ordinal)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS tags_path_sha256 ON tags (path, sha256);
CREATE INDEX IF NOT EXISTS imports_path_sha256 ON imports (path, sha256);
"""


def _has_table(connection: sqlite3.Connection, name: str) -> bool:
    """Whether this database declares ``name`` as a table."""
    row = connection.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?", (name,)
    ).fetchone()
    return row is not None


def _read_meta(connection: sqlite3.Connection) -> _Meta | None:
    """Return the meta row, or None when there is not one to read.

    "There is no meta table" is asked directly rather than inferred from an
    ``OperationalError`` around the SELECT. That class does cover the case
    meant here -- an empty database file, or one from before the table
    existed -- but it also covers a disk I/O error, a database file that
    will not open and a lock wait that timed out. Reading any of those as
    "no usable index" made a healthy index look absent, and the read path
    answers an absent index by unlinking the file. They raise instead, and
    the caller degrades without deleting anything.
    """
    if not _has_table(connection, "meta"):
        return None

    row = connection.execute(
        "SELECT schema_version, repo_root, generation_tree_oid, head_sha FROM meta WHERE id = 1"
    ).fetchone()
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
        return MESSAGES.cache_discarded_no_index
    if meta.schema_version != SCHEMA_VERSION:
        return MESSAGES.cache_discarded_old_schema.format(
            found=meta.schema_version, expected=SCHEMA_VERSION
        )
    if meta.repo_root != str(repo_root):
        return MESSAGES.cache_discarded_other_repo.format(repo_root=meta.repo_root)
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


def _row_counts(connection: sqlite3.Connection) -> _RowCounts:
    """Return how many rows of each kind the database holds."""
    return _RowCounts(
        files=_count(connection, "SELECT COUNT(*) FROM files"),
        tags=_count(connection, "SELECT COUNT(*) FROM tags"),
        imports=_count(connection, "SELECT COUNT(*) FROM imports"),
        refs=_count(connection, "SELECT COUNT(*) FROM refs"),
    )


def _count(connection: sqlite3.Connection, query: str) -> int:
    """Run one COUNT(*) query and return its number."""
    row = connection.execute(query).fetchone()
    return int(row[0])


@contextmanager
def _write_lock(directory: Path, repo_root: Path) -> Iterator[None]:
    """Hold the index write lock, or refuse immediately.

    The platform-specific half lives in :mod:`agentless_mcp.util.filelock`;
    what belongs here is the message, because this is the layer that knows
    which repository the caller was trying to index.
    """
    flavour = platforms.family(sys.platform)
    with ExitStack() as stack:
        try:
            stack.enter_context(filelock.exclusive(directory / LOCK_NAME, flavour=flavour))
        except filelock.LockUnavailableError as exc:
            # The lock is unavailable either because another run holds it or
            # because the filesystem cannot lock at all (ENOLCK/EOPNOTSUPP on
            # some NFS, FUSE and overlay mounts). Naming only the first sends
            # an operator hunting for a process that does not exist, so say
            # both and carry the underlying reason.
            message = (
                f"could not take the index lock for {repo_root}: another index "
                f"run may hold it, or {directory} may be on a filesystem that "
                f"does not support locking"
            )
            raise CacheLocked(message) from exc
        # Outside the try on purpose: the body runs under the lock, and a
        # ``LockUnavailableError`` from a nested lock is not this lock failing
        # to open.
        yield


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
    return digest.hexdigest()[:MANIFEST_DIGEST_LENGTH]


def _absent_status(database: Path | None, note: str = "") -> CacheStatus:
    """Status for a repository with no usable cache."""
    return CacheStatus(
        path=database,
        generation=None,
        repo_generation=None,
        generation_matches=False,
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
        generation_matches=False,
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

    Callers must already hold the write lock. The read path calls
    :func:`_discard_unlocked` instead, which takes it.
    """
    for suffix in ("", "-wal", "-shm"):
        Path(str(database) + suffix).unlink(missing_ok=True)


def _discard_unlocked(database: Path, repo_root: Path) -> None:
    """Discard a database from the read path, under the write lock.

    The write path's identical delete is sound because it holds this lock.
    The read path's was not: two installs of different versions each open the
    other's database, each judge it unusable, and each delete it -- including
    the one the other has just finished writing. Neither ever keeps an index,
    and nothing says why, because discarding is a degradation rather than an
    error.

    A lock that cannot be taken means somebody is writing that file right
    now, and what they are writing is very likely the database this build
    wants. Leaving it is strictly better than deleting it: the caller
    degrades to on-demand parsing either way, and the next call finds a
    finished index rather than a hole.
    """
    try:
        with _write_lock(database.parent, repo_root):
            _discard(database)
    except CacheLocked as exc:
        logger.info(
            "tag cache %s left in place: an index run holds the write lock (%s)", database, exc
        )
    except OSError as exc:
        logger.warning("tag cache %s could not be discarded: %r", database, exc)
