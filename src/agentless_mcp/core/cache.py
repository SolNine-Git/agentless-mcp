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
import shlex
import sqlite3
import sys
import threading
import time
from collections.abc import Callable, Iterator, Sequence
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass, field
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
from agentless_mcp.util.cachedir import (
    DIRECTORY_MODE,
    cache_root,
)
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
#
# v11 drops ``imports.resolved_path``. It was written on every row and read
# only by the code that wrote it back, so nothing ever consulted it; the
# resolver computes the target from ``module`` and the repository's own file
# list. The bump is not optional: a v10 database declares the column
# ``NOT NULL``, so this build's shorter INSERT fails on the first file of
# every index run.
# 12 (2026-08-23): the extraction fixes in 0.5.1. C and C++ method definitions
#   become symbols and carry an owner, so ``tags`` gains rows and
#   ``parent_class`` values it did not have; java, kotlin, scala, php and C#
#   imports record the name they bind, so ``imports.names`` and
#   ``local_names`` change; ``tags.is_public`` changes for TypeScript, TSX,
#   JavaScript and Swift, where a visibility keyword now beats the leading
#   underscore convention.
#
#   The bump exists for a reader who is NOT a released user. Every released
#   build wrote v7 -- versions 8 through 11 were all introduced on this branch
#   and none reached ``main`` -- so a released cache is discarded whether this
#   constant says 11 or 12, and the bump costs those users nothing at all.
#   What it buys is the other case: the staleness check is ``!=``, so a
#   developer who ran an intermediate v11 build of this branch holds a v11
#   cache that 11 would NOT invalidate, and it would go on serving rows the
#   pre-fix extractor wrote. That is the exact failure this package exists to
#   refuse -- a confident answer built from facts nobody re-derived -- and it
#   is worth one rebuild by the handful of people who tested mid-branch.
# 13 (2026-08-26): every row is keyed on ``files.id`` rather than on the
#   ``(path, sha256)`` pair it used to repeat. Measured with ``dbstat`` on the
#   largest cache on one developer machine -- 9,045 files, 2.75M rows -- the
#   repeated key text was 387.6 MB of a 538.4 MB ``refs`` table, and 83.6% of
#   the 643.9 MB file was that one table. Rebuilt against the same data the
#   file is 110.9 MB: 83% smaller, reads 1.20x faster, writes 1.78x faster,
#   and 363,267 rows compared tuple for tuple with no mismatch.
#
#   Every table changes shape, so there is nothing here an older reader could
#   partially understand -- which is exactly what the version gate is for.
#
# 14: the stored role vocabulary shrank. A build that shipped briefly wrote
#   ``self_attribute`` into ``refs.role``, and this build has no such member,
#   so every cached file written by it raised on read and fell back to parsing
#   -- correct, and one warning per file on every call until something
#   re-indexed. The row shape is unchanged, which is the trap: only the
#   version can say the vocabulary moved.
#
# 15: Python extraction gained function-nested ``def`` symbols. The row shape
#   is unchanged and old rows still read, which is again the trap: a cache
#   written before the change would keep serving the smaller symbol set for
#   every unchanged file, and no per-file staleness check can see it.
SCHEMA_VERSION = 15

ENV_NO_AUTO_INDEX = "AGENTLESS_MCP_NO_AUTO_INDEX"
ENV_MAX_CACHE_BYTES = "AGENTLESS_MCP_MAX_CACHE_BYTES"
DATABASE_NAME = "tags.db"
LOCK_NAME = "write.lock"

# The byte ceiling on the whole cache root, enforced after every index run.
#
# There was no ceiling before, and the cache root is written by auto-indexing
# rather than by a command anybody types, so nothing bounded it. Measured on
# one developer machine on 2026-08-26: 490 databases, 5.67 GB, the largest
# 644 MB and the median under a megabyte. The mass is in a few dozen entries
# and the long tail is nearly free, which is why this is a size cap and not an
# age cap -- on that same machine nothing was older than fourteen days, so any
# defensible age rule would have evicted nothing at all while the directory
# kept growing.
#
# 5 GiB. Two measurements set it. It holds the largest single repository
# observed (644 MB) with room to spare, and it clears the largest working set
# measured here -- a 60-repository benchmark whose caches total 4.66 GiB, which
# a 4 GiB ceiling could not hold and thrashed against. It also sits under the
# 5.67 GB the unbounded directory had reached, so the sweep still has work to
# do. Set ``AGENTLESS_MCP_MAX_CACHE_BYTES`` to another value, or to 0 to keep
# the old unbounded behaviour.
DEFAULT_MAX_CACHE_BYTES = 5 * 1024**3

# How recently a database must have been used to be exempt from the sweep.
#
# The ceiling alone was wrong, and measurably so. It treats hot and cold
# databases alike, so a working set larger than the ceiling makes each newly
# indexed repository evict one that is about to be needed again -- and every
# eviction costs a full re-index, which is the exact work the cache exists to
# remove. Measured on 2026-08-26: a 60-repository benchmark whose caches total
# 4.66 GiB ran against the 4.00 GiB ceiling, drove the database count from 239
# to 102 mid-run, evicted 8 of the 60 repositories it was actively using, and
# came out 37.0s -> 49.0s per instance against the same benchmark on the
# unbounded build. Thrash, not reclamation.
#
# The two populations are nothing alike, which is what makes a window work. On
# the machine that motivated the ceiling, 430 of 490 databases had gone
# untouched for over a day while the benchmark touched all 60 of its own inside
# one hour. A day-long window would have released essentially all of the dead
# weight and none of the live set.
#
# So the ceiling bounds cold accumulation and never arbitrates between hot
# entries. A working set that exceeds it on its own exceeds it: deleting a
# database somebody is still using costs more than it reclaims.
PROTECTED_SECONDS = 24 * 60 * 60

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
    # The repository this cache describes, or None when there is no cache to
    # describe. Only the generation-mismatch receipt reads it, and that branch
    # requires a generation, which the absent and bypassed statuses do not
    # have. Appended with a default rather than placed by meaning: this is a
    # shipped dataclass and a positional insertion would break its callers.
    repo_root: Path | None = None

    def __post_init__(self) -> None:
        """Refuse a status that describes a cache without naming its repository.

        The type cannot say "optional, except when there is a generation", so
        the pairing is checked here instead of trusted to the construction
        sites. Held at construction rather than in :attr:`receipt`, because a
        receipt that discovers the gap has no honest line to print: it would
        either raise from a status line or recommend
        ``agentless-mcp index --repo None``.
        """
        if self.generation is not None and self.repo_root is None:
            message = "a cache status carrying a generation must name its repository"
            raise ValueError(message)

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
        remediation = (
            STALE_REFRESHING
            if refreshing
            else REMEDIATION.format(repo_root=shlex.quote(str(self.repo_root)))
        )
        return (
            f"g:{self.generation} generation mismatch (repo g:{self.repo_generation}); "
            f"{remediation}"
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
    """The digest and grammar version one indexed file was recorded with.

    ``file_id`` is excluded from equality on purpose. The two sites that
    compare an entry are asking one question -- same content, same grammar --
    and the row identifier is not part of it. Comparing it would make every
    reuse check fail against a freshly loaded entry and re-extract the whole
    repository on every run. The default is 0, which no AUTOINCREMENT id can
    ever be, so a constructed-for-comparison entry cannot alias a real row.
    """

    digest: str
    grammar_version: str
    file_id: int = field(compare=False, default=0)


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
    # The repository this cache was built for. Carried so the stale receipt
    # can name it in the command it recommends: the receipt is read by an
    # agent whose working directory is often not the repository, and
    # ``agentless-mcp index`` with no argument then indexes the wrong tree
    # or refuses.
    repo_root: Path


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
        read: "Callable[[str, int], list[Any]]",
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
        file_id = self._fresh_file_id(text, path)
        if file_id is None:
            return None
        try:
            return read(path, file_id)
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
            repo_root=self._state.repo_root,
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

    def _fresh_file_id(self, text: str, path: str) -> int | None:
        """Return the id to read rows at, or None when they cannot be used.

        One gate for all three row kinds: the file must be indexed, indexed by
        the grammar pack that is installed now, and indexed from exactly this
        content. Anything else and the caller parses.

        Returning the id rather than the digest is what lets the fact tables
        stop repeating the digest on every row without weakening anything. The
        id is reachable only through this function and only when the content
        matched, so a caller that skips the check has no key to query with.
        The guarantee is unchanged; it is simply established once per file
        instead of once per row.
        """
        entry = self._state.entries.get(path)
        if entry is None or entry.grammar_version != self._state.grammar_version:
            return None
        return entry.file_id if entry.digest == content_digest(text) else None

    def _symbol_rows(self, path: str, file_id: int) -> list[ASTSymbol]:
        """Rebuild one file's symbols from its tag rows, in extraction order.

        The collision ordinal is recomputed rather than stored: it is a pure
        function of one file's symbol list, the rows come back in the order
        the extractor produced them, and deriving it here is what guarantees a
        cached answer and an on-demand one carry the same stable ids.
        """
        cursor = self._connection.execute(
            "SELECT name, kind, start_line, end_line, signature, parent, docstring, "
            "decorators, bases, language, is_public, is_async, rationales "
            "FROM tags WHERE file_id = ? ORDER BY ordinal",
            (file_id,),
        )
        return disambiguate([_symbol_from_row(row, path) for row in cursor.fetchall()])

    def _import_rows(self, _path: str, file_id: int) -> list[ImportStatement]:
        """Rebuild one file's import statements from its rows, in extraction order.

        Takes the path it does not use because :meth:`_rows_or_none` calls all
        three readers through one signature. The other two need it: symbols to
        build their stable ids, references to name the file they were found in.
        """
        cursor = self._connection.execute(
            "SELECT module, names, is_relative, relative_level, line, "
            "binds_all, alias, local_names FROM imports "
            "WHERE file_id = ? ORDER BY ordinal",
            (file_id,),
        )
        return [_import_from_row(row) for row in cursor.fetchall()]

    def _ref_rows(self, path: str, file_id: int) -> list[Ref]:
        """Rebuild one file's identifier references from its rows, in order."""
        cursor = self._connection.execute(
            "SELECT name, line, role, qualifier FROM refs WHERE file_id = ? ORDER BY ordinal",
            (file_id,),
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

    # Stamped here, at the one point that proves the index was both usable and
    # actually used: past the schema, repository and generation checks, and
    # about to be handed to a caller as its source of facts. Stamping on entry
    # would keep a database alive for being probed and rejected.
    touch_database(database)

    return CachedSource(
        connection,
        extractor,
        _CacheState(
            database=database,
            entries=opened.entries,
            generation=meta.generation,
            repo_generation=repo_generation(repo_root, tree_oid),
            grammar_version=grammars.pack_version(),
            repo_root=repo_root,
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


# The sidecars a database is accounted for and deleted with. One tuple rather
# than the literal repeated in ``_discard`` and ``_entry_bytes``, because the
# sweep's arithmetic is a promise about what the delete reclaims: if the two
# lists drift, the report says it freed bytes that are still on the disk.
#
# Membership is the promise; this order is not. ``_entry_bytes`` sums, so it
# reads the tuple either way, and ``_discard`` deliberately walks it reversed
# for the reason given there. The primary is spelled first here because that is
# how SQLite names the set, not because anything may delete in this order.
DATABASE_SUFFIXES = ("", "-wal", "-shm")


@dataclass(frozen=True)
class _CacheEntry:
    """One repository's cached database, as the sweep weighs it.

    ``used_at`` is the database's mtime, which :func:`touch_database` refreshes
    on every successful open. Without that refresh it would mean "last
    written", and a repository that is read constantly but has not changed in
    a month is exactly the one an index is most worth keeping.
    """

    database: Path
    size: int
    used_at: float


@dataclass(frozen=True)
class EvictionReport:
    """What one sweep of the cache root deleted."""

    databases: int
    size: int

    @property
    def happened(self) -> bool:
        """True when the sweep deleted anything."""
        return self.databases > 0


NOTHING_EVICTED = EvictionReport(databases=0, size=0)


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
    # What the ceiling sweep deleted from other repositories after this run.
    # Appended with a default rather than placed by meaning, for the reason
    # ``CacheStatus.repo_root`` gives: this is a shipped dataclass and a
    # positional insertion would break its callers.
    evicted: EvictionReport = NOTHING_EVICTED

    @property
    def errors(self) -> int:
        """How many files could not be recorded."""
        return len(self.failures)

    @property
    def skipped(self) -> int:
        """How many known-language files were recorded without facts."""
        return len(self.skipped_files)

    def summary_line(self) -> str:
        """Return the one-line summary the CLI prints.

        The eviction clause appears only when a sweep deleted something.
        Printing "evicted 0" on every run would train a reader to skip the
        field, and this is the one number on the line that describes a
        deletion outside the repository being indexed.
        """
        eviction_clause = (
            f", evicted {self.evicted.databases} cached repositories ({self.evicted.size} bytes)"
            if self.evicted.happened
            else ""
        )
        return (
            f"indexed {self.indexed}, reused {self.reused}, pruned {self.pruned}, "
            f"skipped {self.skipped}, errors {self.errors}: {self.files} files, "
            f"{self.tags} tags, {self.imports} imports, {self.refs} refs "
            f"at g:{self.generation} in {self.database}{eviction_clause}"
        )

    def as_dict(self) -> dict[str, Any]:
        """Return the JSON form of this report.

        ``evicted`` is always present here, unlike in the summary line: a JSON
        consumer reads a key it looked up, so a field that appears only
        sometimes is a field it has to guard for no gain.
        """
        return {
            "database": str(self.database),
            "generation": self.generation,
            "indexed": self.indexed,
            "reused": self.reused,
            "pruned": self.pruned,
            "evicted": {
                "databases": self.evicted.databases,
                "bytes": self.evicted.size,
            },
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


def max_cache_bytes() -> int | None:
    """Return the byte ceiling on the cache root, or None when it is disabled.

    ``0`` disables the sweep and restores the unbounded behaviour, which is
    the reason this returns an option rather than a very large number: "do not
    enforce a ceiling" and "enforce a ceiling of zero" are different
    instructions, and the second one would delete every database on the
    machine after every index run.

    A value that is not a non-negative integer is reported and ignored. The
    alternative -- reading a typo as "no ceiling" -- is the failure mode this
    whole function exists to remove, and it would be invisible: the sweep
    would simply never fire and the directory would grow exactly as it did
    before anybody set the variable.
    """
    configured = os.environ.get(ENV_MAX_CACHE_BYTES, "").strip()
    if not configured:
        return DEFAULT_MAX_CACHE_BYTES
    try:
        limit = int(configured)
    except ValueError:
        limit = -1
    if limit < 0:
        logger.warning(
            "%s=%r is not a non-negative integer and was ignored; the default ceiling of "
            "%d bytes applies. Set it to 0 to disable the sweep",
            ENV_MAX_CACHE_BYTES,
            configured,
            DEFAULT_MAX_CACHE_BYTES,
        )
        return DEFAULT_MAX_CACHE_BYTES
    return limit or None


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

    Unreadable is logged rather than answered silently. "Not current" starts a
    full background index of the repository, and nothing downstream could tell
    that decision apart from an ordinary stale stamp: both produce the same
    refresh, and only one of them says the storage is failing.
    """
    if not database.exists():
        return False
    try:
        connection = _connect(database)
    except sqlite3.DatabaseError as exc:
        logger.warning(
            "tag cache %s could not be opened; treating it as stale and refreshing: %r",
            database,
            exc,
        )
        return False
    try:
        meta = _read_meta(connection)
    except sqlite3.DatabaseError as exc:
        logger.warning(
            "tag cache %s could not be read; treating it as stale and refreshing: %r",
            database,
            exc,
        )
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

    # After the lock is released, not inside it. The sweep takes the write
    # lock of every database it deletes, and this run's own lock is one it
    # would otherwise be holding while asking for it again.
    evicted = enforce_cache_limit(keep=database)

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
        evicted=evicted,
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
            # Children before the parent, and by the id the parent owns. The
            # order is load-bearing rather than tidy: the fact rows are keyed
            # on ``files.id``, so deleting the ``files`` row first would take
            # away the only key that reaches them and leave them orphaned in a
            # table nothing would ever clean.
            #
            # A path absent from ``previous`` is a file this index has never
            # recorded, so it owns no id and no rows. Its ``files`` delete is
            # still issued, because ``vanished`` and ``touched`` are the paths
            # to clear and not the paths known to be present.
            recorded = previous.get(path)
            if recorded is not None:
                # Spelled out rather than looped over a table-name list: a
                # query built by interpolating a name is the shape of an
                # injection even when today's names are constants.
                connection.execute("DELETE FROM tags WHERE file_id = ?", (recorded.file_id,))
                connection.execute("DELETE FROM imports WHERE file_id = ?", (recorded.file_id,))
                connection.execute("DELETE FROM refs WHERE file_id = ?", (recorded.file_id,))
            connection.execute("DELETE FROM files WHERE path = ?", (path,))

        for entry in plan.writes:
            file_id = _insert_file(connection, entry)
            connection.executemany(
                "INSERT INTO tags (file_id, ordinal, name, kind, start_line, end_line, "
                "signature, parent, docstring, decorators, bases, language, "
                "is_public, is_async, rationales) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    _tag_row(file_id, ordinal, symbol)
                    for ordinal, symbol in enumerate(entry.symbols)
                ],
            )
            connection.executemany(
                "INSERT INTO imports (file_id, ordinal, module, names, is_relative, "
                "relative_level, line, binds_all, alias, local_names) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    _import_row(file_id, ordinal, statement)
                    for ordinal, statement in enumerate(entry.imports)
                ],
            )
            connection.executemany(
                "INSERT INTO refs (file_id, ordinal, name, line, role, qualifier) "
                "VALUES (?, ?, ?, ?, ?, ?)",
                [
                    (file_id, ordinal, ref.name, ref.line, ref.role.value, ref.qualifier)
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


def _insert_file(connection: sqlite3.Connection, entry: _FileTags) -> int:
    """Insert one file's row and return the id its fact rows are keyed on.

    Read from the insert that minted it rather than selected back:
    AUTOINCREMENT guarantees the id is new, and a second query would only be a
    slower way to learn what the cursor already knows.

    ``lastrowid`` is typed optional because it is None for statements that
    insert nothing, which an INSERT that did not raise is not. Asserted rather
    than defaulted: a zero or a None flowing into ``file_id`` would key every
    one of this file's rows onto an id no ``files`` row owns, and they would
    read back as somebody else's symbols rather than as an error.
    """
    cursor = connection.execute(
        "INSERT INTO files (path, sha256, grammar_version) VALUES (?, ?, ?)",
        (entry.path, entry.digest, entry.grammar_version),
    )
    file_id = cursor.lastrowid
    if file_id is None:
        unreachable = "an INSERT that did not raise always reports a row id"
        raise AssertionError(unreachable)
    return file_id


def _tag_row(file_id: int, ordinal: int, symbol: ASTSymbol) -> tuple[Any, ...]:
    """Flatten one symbol into its tag row."""
    return (
        file_id,
        ordinal,
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
    )


def _import_row(file_id: int, ordinal: int, statement: ImportStatement) -> tuple[Any, ...]:
    """Flatten one import statement into its row."""
    return (
        file_id,
        ordinal,
        statement.module,
        json.dumps(list(statement.names)),
        int(statement.is_relative),
        statement.relative_level,
        statement.line_number,
        int(statement.binds_all),
        statement.alias,
        json.dumps(list(statement.local_names)),
    )


def _import_from_row(row: Sequence[Any]) -> ImportStatement:
    """Rebuild one import statement from its row, converting explicitly."""
    return ImportStatement(
        module=str(row[0]),
        names=tuple(str(item) for item in json.loads(str(row[1]))),
        is_relative=bool(row[2]),
        relative_level=int(row[3]),
        line_number=int(row[4]),
        binds_all=bool(row[5]),
        alias=str(row[6]),
        local_names=tuple(str(item) for item in json.loads(str(row[7]))),
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
    """Create the schema, dropping a database written by another version.

    A meta row that cannot be read propagates. It means the file is unusable
    rather than new, and :func:`_open_for_write` answers exactly that by
    discarding it and rebuilding -- the recovery its own docstring describes,
    and which a handler here made unreachable by reporting every unreadable
    database as one with no meta row.
    """
    meta = _read_meta(connection)
    stale = meta is not None and meta.schema_version != SCHEMA_VERSION
    connection.executescript(_MIGRATE_SCHEMA if stale else _CREATE_SCHEMA)
    if stale:
        _reclaim(connection)


def _reclaim(connection: sqlite3.Connection) -> None:
    """Return a migrated database's freed pages to the filesystem.

    Dropping a table frees its pages inside the file and never shrinks the
    file: SQLite keeps them on a freelist and spends them on the next thing
    that grows. Without this, a migration that genuinely shrinks the data
    leaves the old size on disk. Measured on the largest cache here when v13
    landed -- 643.9 MB of file holding 125.2 MB of data and 518.8 MB of
    freelist, so the entire storage change was invisible.

    It matters twice over, because the size the cache ceiling reads is the
    size on disk. A database that never gives its pages back is counted at its
    high-water mark forever, and would be evicted for space it is not using.

    Outside a transaction, because VACUUM cannot run inside one --
    ``executescript`` has already committed the migration by the time this
    runs. Failure is reported and not raised: the migration itself succeeded,
    the database is correct, and a file that is larger than it needs to be is
    a cost rather than a fault. The usual cause is the one worth naming, which
    is why the message names it -- VACUUM rebuilds the database beside itself
    and needs room for a second copy.
    """
    try:
        connection.execute("VACUUM")
    except sqlite3.DatabaseError as exc:
        logger.warning(
            "could not reclaim free pages after the schema migration (%r); the database is "
            "correct but still holds the space the old schema used. A VACUUM needs free disk "
            "for a second copy of the file",
            exc,
        )


# A version bump drops every table this package owns and rebuilds them. The
# list is exhaustive on purpose: a table left behind by an older schema would
# be read by the new code as if the new code had written it. The order within
# it carries no meaning: :data:`_MIGRATE_SCHEMA` runs the drop and the create
# as one transaction, so no reader and no later run can observe a database
# that is part one version and part the other.
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
-- ``id`` is what every other table keys on, and it is AUTOINCREMENT rather
-- than a plain rowid on purpose. A plain rowid is reused after a delete, so a
-- newly indexed file could inherit the id of one just removed -- and if any
-- child row had survived that delete it would be served for the wrong file,
-- silently, with the digest gate satisfied because the gate reads this table.
-- The monotonic counter costs one row in ``sqlite_sequence`` and removes the
-- whole class.
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    sha256 TEXT NOT NULL,
    grammar_version TEXT NOT NULL
);
-- The three fact tables share one shape: keyed on ``(file_id, ordinal)``,
-- WITHOUT ROWID, no secondary index. Every read of them is "one file, in
-- extraction order", so that key clusters the rows the way they are read and
-- the primary key alone answers the query.
--
-- They used to repeat ``(path, sha256)`` on every row and carry a separate
-- index over it. On one real repository that was 387.6 MB of duplicated path
-- and digest text in ``refs`` alone, plus 42.6 MB of index that the clustering
-- key now makes redundant. The digest did not have to live here to be
-- load-bearing: ``CachedSource._fresh_file_id`` checks it once per file, and a
-- caller whose content does not match never obtains an id to query with.
CREATE TABLE IF NOT EXISTS tags (
    file_id INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
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
    PRIMARY KEY (file_id, ordinal)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS imports (
    file_id INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    module TEXT NOT NULL,
    names TEXT NOT NULL,
    is_relative INTEGER NOT NULL,
    relative_level INTEGER NOT NULL,
    line INTEGER NOT NULL,
    binds_all INTEGER NOT NULL,
    alias TEXT NOT NULL,
    local_names TEXT NOT NULL,
    PRIMARY KEY (file_id, ordinal)
) WITHOUT ROWID;
-- By far the largest table: millions of rows per repository against thousands
-- of symbols, which is why the key it repeats is the one that decides the size
-- of the whole file.
CREATE TABLE IF NOT EXISTS refs (
    file_id INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    name TEXT NOT NULL,
    line INTEGER NOT NULL,
    role TEXT NOT NULL,
    qualifier TEXT NOT NULL,
    PRIMARY KEY (file_id, ordinal)
) WITHOUT ROWID;
"""

# The two scripts :func:`_ensure_schema` runs, each one transaction.
#
# A migration that is not atomic recovers only by luck. The drop is five
# statements and the create is five more, and in autocommit each of them
# commits on its own; an interrupted run then leaves a database that is part
# one schema and part the other, whose surviving ``meta`` row -- if the drop
# happened to reach it -- decides whether the next run repairs the file or
# builds on top of the wreckage with ``CREATE TABLE IF NOT EXISTS``. Wrapped,
# there is no such state to land in: the file is the old database or the new
# one, and an interrupted migration is simply re-run.
#
# The BEGIN and the COMMIT are inside the script rather than around the call
# because ``executescript`` commits any pending transaction before it starts
# and performs no other transaction control of its own, so a ``BEGIN`` issued
# on the connection first would be committed away rather than honoured.
_BEGIN = "BEGIN;\n"
_COMMIT = "COMMIT;\n"

_CREATE_SCHEMA = _BEGIN + _SCHEMA + _COMMIT
_MIGRATE_SCHEMA = _BEGIN + _DROP_SCHEMA + _SCHEMA + _COMMIT


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
    """Return every indexed file's digest and grammar version.

    No handler on the SELECT, deliberately. The table exists at both call
    sites: the read path reaches this only after :func:`_rejection` accepted a
    meta row of this schema version, and the write path only after
    :func:`_ensure_schema` created it. So what a handler here could catch is
    the environment failing -- a disk I/O error, a database that will not
    open, a lock wait that timed out -- and never an old or missing table.

    Answering one of those with an empty mapping made a healthy index read as
    a repository with no indexed files: every digest missed, every file was
    re-parsed, and the receipt still said ``fresh``. They propagate instead,
    to :func:`_open_indexed`, which reports ``cache unreadable`` and leaves
    the file in place. This is the same rule :func:`_read_meta` states.
    """
    rows = connection.execute("SELECT path, sha256, grammar_version, id FROM files").fetchall()
    return {
        str(row[0]): _FileEntry(
            digest=str(row[1]), grammar_version=str(row[2]), file_id=int(row[3])
        )
        for row in rows
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


def _discard(database: Path) -> bool:
    """Delete a database this build cannot use, with its WAL sidecars.

    Deleting rather than ignoring: the file is derived data, and leaving an
    unusable one behind would make every later call pay the same open, fail
    the same check and report the same degradation.

    Returns whether the primary file was there to delete, which is what lets
    the ceiling sweep report deletions it actually performed. Two sweeps can
    plan the same cold victim, and counting every pass that did not raise made
    the second run's :class:`EvictionReport` claim databases and bytes that
    the first had already reclaimed. The unlink is the test rather than an
    ``exists()`` before it: a check and then an act is the race this return
    value exists to close. Because the primary goes last, the answer also
    implies the primary is gone: the return is only reached once every unlink
    has succeeded, so no caller is handed a value that misdescribes the disk.

    Sidecars are unlinked whatever the primary's fate, and a missing one is
    not a fault -- a database checkpointed since the caller last looked has no
    ``-wal``. Every other :class:`OSError` still propagates, as it did when
    this loop passed ``missing_ok=True``, which swallowed exactly the same one
    exception.

    Callers must already hold the write lock. The read path calls
    :func:`_discard_unlocked` instead, which takes it. The two recovery
    callers ignore the return: whether the unusable file was still there says
    nothing about the rebuild that follows it.
    """
    existed = False
    # Sidecars first and the primary last. The primary is the only file that
    # keeps a cache directory reachable: :func:`_cache_entries` keys each
    # directory on a successful stat of it, so deleting it first and then
    # failing on a ``-wal`` strands those bytes for good, uncounted by the
    # ceiling and unreachable by every later sweep. Reversed, a sidecar that
    # will not go leaves the primary in place, so the entry stays visible and
    # still weighed, and the next sweep attempts the whole delete again. That
    # orders the failure rather than promising it clears: an ``OSError`` here
    # is often persistent, and a repeated warning about a directory the sweep
    # can still see beats silently leaked bytes it never can.
    for suffix in reversed(DATABASE_SUFFIXES):
        try:
            Path(str(database) + suffix).unlink()
        except FileNotFoundError:
            continue
        if not suffix:
            existed = True
    return existed


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


def _entry_bytes(database: Path) -> int:
    """Return what deleting ``database`` and its sidecars would reclaim.

    A missing sidecar contributes nothing rather than raising: a database
    checkpointed since the scan has no ``-wal``, and that is the normal
    resting state, not a fault.
    """
    total = 0
    for suffix in DATABASE_SUFFIXES:
        try:
            total += Path(str(database) + suffix).stat().st_size
        except OSError:
            continue
    return total


def _cache_entries(root: Path) -> list[_CacheEntry]:
    """List every cached database under ``root``, with its size and last use.

    A directory whose database has already gone contributes nothing: the sweep
    leaves ``write.lock`` behind (see :func:`enforce_cache_limit`), so an
    evicted repository stays in the listing as an empty directory and must not
    be weighed or deleted a second time.
    """
    entries: list[_CacheEntry] = []
    try:
        candidates = sorted(root.iterdir())
    except OSError as exc:
        # Not fatal and not silent. The sweep is an optimization on an
        # optimization; a cache root that cannot be listed means no eviction
        # this run, and the run that produced a usable index still succeeded.
        logger.warning("cache root %s could not be listed, so nothing was evicted: %r", root, exc)
        return []

    for directory in candidates:
        database = directory / DATABASE_NAME
        try:
            used_at = database.stat().st_mtime
        except OSError:
            continue
        entries.append(_CacheEntry(database=database, size=_entry_bytes(database), used_at=used_at))
    return entries


def _eviction_plan(
    entries: Sequence[_CacheEntry], limit: int, *, keep: Path, now: float
) -> list[_CacheEntry]:
    """Return the databases to delete, in the order the sweep should delete them.

    Least recently used first, which is the whole policy: walk the entries
    most-recent first, keep them while the running total fits under ``limit``,
    and evict the rest.

    Nothing used within :data:`PROTECTED_SECONDS` of ``now`` is a candidate,
    whatever that does to the total. That is the rule that makes the sweep safe
    to run against a live machine rather than only against an idle one: an
    entry still in use is not spare capacity, and reclaiming it buys bytes at
    the price of the re-index it forces. The constant carries the measurement.

    Two further entries are never candidates, and both key on identity rather
    than on a clock, so they still hold when mtimes do not -- a filesystem that
    does not keep them, a tree restored from an archive, a skewed clock.
    ``keep`` is the database the calling index run just built. The most
    recently used entry is spared from the other direction: without it, one
    repository bigger than the whole ceiling would be evicted by any other
    repository's sweep and rebuilt on its next call, forever.

    The ceiling is therefore a bound on what has gone cold, not a hard cap. It
    is exceeded on purpose whenever the live set alone fills it, and
    :func:`enforce_cache_limit` says so rather than evicting into a live set.
    """
    ordered = sorted(entries, key=lambda entry: entry.used_at, reverse=True)
    protected_after = now - PROTECTED_SECONDS
    evict: list[_CacheEntry] = []
    total = 0
    for position, entry in enumerate(ordered):
        if entry.database == keep or position == 0 or entry.used_at >= protected_after:
            total += entry.size
            continue
        if total + entry.size <= limit:
            total += entry.size
            continue
        evict.append(entry)
    return evict


def _evict_victim(entry: _CacheEntry, *, now: float) -> int | None:
    """Delete one planned victim, returning the bytes the delete reclaimed.

    ``None`` means the sweep must not count this entry, for one of two
    reasons.

    It was already gone. A concurrent sweep planned the same cold victim and
    reached it first, and the run that arrives second has reclaimed nothing to
    report.

    Or it stopped being cold. :func:`_eviction_plan` decides protection from
    the mtimes :func:`_cache_entries` read before any lock was taken, and the
    read path stamps a database through :func:`touch_database` holding no lock
    at all, so an entry can go from cold to hot between the plan and this
    call. Re-reading the mtime here is what makes the documented 24 hour
    protection hold rather than merely usually hold.

    The re-read narrows that window to one stat and one unlink. It does not
    close it, and closing it would mean the readers and the sweep coordinating.
    They deliberately do not: :func:`enforce_cache_limit` gives the reason an
    eviction is a cost and never a wrong answer, and a reader that loses this
    race pays one re-index.

    A failed stat is the already-gone signal rather than a fault, so nothing is
    warned about it. Every other :class:`OSError` reaches the caller's existing
    warning path.

    The caller holds this victim's write lock, as :func:`_discard` requires.
    """
    try:
        used_at = entry.database.stat().st_mtime
    except OSError:
        return None

    if used_at >= now - PROTECTED_SECONDS:
        logger.info(
            "kept %s: a read stamped it as used after the sweep planned its eviction",
            entry.database,
        )
        return None

    # Measured now rather than taken from ``entry.size``, for the reason
    # ``DATABASE_SUFFIXES`` gives: the figure is a promise about bytes that
    # left the disk. A size read before the lock drifts for the same reasons
    # the mtime does, so re-reading one and trusting the other would be half a
    # fix. Three stats against the unlink they precede cost nothing.
    freed = _entry_bytes(entry.database)
    if not _discard(entry.database):
        return None
    return freed


def enforce_cache_limit(*, keep: Path) -> EvictionReport:
    """Delete least-recently-used databases until the cache root fits its ceiling.

    Called after an index run rather than on the read path, because indexing is
    the only thing that makes the cache grow and the read path is the hot one.
    That also makes the owner of the growth the owner of the bound.

    Evicting a cache is always safe, which is what lets this run without
    coordinating with readers. A reader that opens a database after the delete
    finds nothing, reports ``cache: none`` and parses on demand -- the same
    degradation the package already takes for a missing, corrupt or
    older-schema index -- and a reader holding the file open keeps reading the
    unlinked inode until it closes. The write lock is still taken per victim,
    so a concurrent index run is never deleted out from underneath.

    A ceiling the live set alone exceeds is reported and not enforced. That is
    the whole correction to the first version of this sweep, which enforced it
    by evicting databases it was about to need again -- see
    :data:`PROTECTED_SECONDS` for what that cost when measured.

    The returned counts describe what this run deleted and nothing else. The
    plan is computed from one unlocked scan, so a victim can be gone or hot
    again by the time its turn comes; :func:`_evict_victim` re-checks each one
    under its lock, and only what it confirms is counted.

    ``write.lock`` and the directory holding it stay.
    :mod:`agentless_mcp.util.filelock` documents why the lock file is never
    unlinked: a second process would create and lock a *different* file of the
    same name while the first still holds the old one, and the mutual exclusion
    the index depends on would quietly stop existing. An empty directory with a
    zero-byte lock file costs an inode, and this sweep is about gigabytes.
    """
    limit = max_cache_bytes()
    if limit is None:
        return NOTHING_EVICTED

    # One clock for the plan and for every re-check it feeds, so a sweep long
    # enough to matter cannot protect a later victim by a rule it did not
    # apply to an earlier one.
    now = time.time()
    entries = _cache_entries(cache_root())
    victims = _eviction_plan(entries, limit, keep=keep, now=now)
    if not victims:
        _report_unreclaimable(entries, limit)
        return NOTHING_EVICTED

    deleted = 0
    freed = 0
    for entry in victims:
        # The lock's subject is the cache directory, not a repository: the
        # sweep never resolves the repository a victim describes, and reading
        # it back out of ``meta`` would mean opening every database it is
        # about to delete.
        try:
            with _write_lock(entry.database.parent, entry.database.parent):
                reclaimed = _evict_victim(entry, now=now)
        except CacheLocked as exc:
            logger.info("kept %s: an index run holds its write lock (%s)", entry.database, exc)
            continue
        except OSError as exc:
            logger.warning("could not evict %s: %r", entry.database, exc)
            continue
        if reclaimed is None:
            continue
        deleted += 1
        freed += reclaimed

    if deleted:
        logger.info(
            "cache root trimmed to its %d byte ceiling: %d databases deleted, %d bytes freed",
            limit,
            deleted,
            freed,
        )
    return EvictionReport(databases=deleted, size=freed)


def _report_unreclaimable(entries: Sequence[_CacheEntry], limit: int) -> None:
    """Say when the cache root is over its ceiling with nothing cold to release.

    Silence here would be the first version's mistake wearing a different
    face. That build met the ceiling by evicting live databases, and the only
    signal was a benchmark that got slower. This one keeps them and says why,
    so the condition is a line in a log rather than an unexplained slowdown.
    """
    total = sum(entry.size for entry in entries)
    if total <= limit:
        return
    logger.info(
        "cache root is %d bytes against a %d byte ceiling and nothing is cold enough to "
        "release: %d databases were all used within the last %d seconds. Evicting one would "
        "force a re-index of a repository still in use, which costs more than it reclaims",
        total,
        limit,
        len(entries),
        PROTECTED_SECONDS,
    )


def touch_database(database: Path) -> None:
    """Record that ``database`` was used now, for the eviction sweep's ordering.

    One ``utime`` on a file the caller has already opened, which is what turns
    :func:`enforce_cache_limit` from least-recently-*written* into least
    recently *used*. Without it a repository nobody has committed to in a month
    looks abandoned to the sweep however often it is read, and its index -- the
    expensive one to rebuild -- is the first thing deleted.

    A failure degrades the ordering and nothing else, so it is logged at debug
    and not raised: a cache on a read-only mount cannot grow, so a sweep that
    mis-orders it has nothing to delete anyway.
    """
    try:
        os.utime(database)
    except OSError as exc:
        logger.debug("could not stamp %s as used: %r", database, exc)
