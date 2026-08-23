"""The MCP server: the read tools, over the same application services.

The transport is the operator's choice and stdio is the default, because the
client that launches this process as a child is the shape every registered
client uses. ``--transport http`` serves the same tools over FastMCP's
streamable-http transport instead, for a client that cannot spawn a child --
one long-lived server several clients share. The tools, the allowlist and the
refusals are identical either way: only the pipe changes.

This adapter owns two things the CLI does not, and nothing else.

**The allowlist.** One server process serves a workspace of repositories, so
there is no cwd to infer a root from and inferring one would be a
wrong-repository answer. Every tool therefore takes ``repo_root`` first and it
is checked, exactly, against the roots the server was started with. Those come
from repeatable ``--root DIR`` flags, and from ``--roots-from FILE`` which is
that same list written one path per line, and from nowhere else. The flags are
fixed for the process lifetime; the file is the operator's editable half of the
allowlist, re-read whenever it changes on disk, so appending a line enrols a
repository on the next call without a restart. The client's own
MCP ``roots`` capability -- verified present in the installed FastMCP as
``Context.list_roots()`` -- is read, but an advertised root can only *select*
among the configured ones, never add one. A server started with no configured root
serves nothing, whatever the client advertises, because otherwise the client
rather than the operator would be deciding what this process may read. A
client that does not implement roots answers "List roots not supported"; that
is a normal negative, not a failure, and the static roots still apply.

**The refusal on ambiguity.** With exactly one configured root, an omitted
``repo_root`` defaults to it -- there is nothing to be ambiguous about. The
client's advertised roots select the same way: when the advertised workspace
picks out exactly one configured root -- equal to it, or nested either way
round -- an omitted ``repo_root`` defaults to that root, receipted like any
other answer. With several candidates left, or none, an omitted or unmatched
root is refused with the list of allowed roots rather than guessed at.

Everything else is a thin call into the same services the CLI uses. There are
no write, exec or fetch tools here and there will not be: patch application
and test execution are CLI-side behind a git worktree, and grammar downloads
happen in ``warmup``, never inside a tool call.
"""

import argparse
import asyncio
import ipaddress
import logging
import shlex
import socket
import sys
import threading
from collections.abc import AsyncIterator, Callable, Mapping, Sequence
from contextlib import AbstractAsyncContextManager, asynccontextmanager
from dataclasses import dataclass, field, replace
from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as distribution_version
from itertools import chain
from pathlib import Path
from typing import Annotated, Literal
from urllib.parse import unquote, urlparse

from fastmcp import Context, FastMCP
from fastmcp.exceptions import FastMCPError, ToolError
from fastmcp.server.middleware import CallNext, Middleware, MiddlewareContext
from fastmcp.tools import ToolResult
from mcp import types as mcp_types
from mcp.shared.exceptions import McpError
from pydantic import Field, ValidationError

from agentless_mcp.adapters.mcp.annotations import read_only
from agentless_mcp.application import envelope, render
from agentless_mcp.application.capability_service import (
    build_capability_report,
    render_capability_report,
)
from agentless_mcp.application.graph_service import (
    DEFAULT_COMMUNITY_LIMIT,
    DEFAULT_CYCLE_LIMIT,
    DEFAULT_EXPLAIN_LIMIT,
    GraphService,
    PathOptions,
)
from agentless_mcp.application.map_service import MapRequest, MapService
from agentless_mcp.application.repo_context import RepoContext, resolve_repo, resolved_allowlist
from agentless_mcp.application.symbol_service import (
    DEFAULT_EXPAND_LIMIT,
    DEFAULT_FIND_LIMIT,
    DEFAULT_REFS_LIMIT,
    SymbolService,
    render_expansion,
    render_find,
)
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import cache, grammars, projectconfig, selfrestart
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.locs import DEFAULT_CONTEXT_LINES
from agentless_mcp.core.mermaid import DEFAULT_DIAGRAM_NODES
from agentless_mcp.core.symbols import SymbolKind, stable_id
from agentless_mcp.core.treewalk import DEFAULT_MAX_ENTRIES, DEFAULT_RENDER_DEPTH
from agentless_mcp.prompts import MESSAGES, PARAMETER_DESCRIPTIONS, TOOL_DESCRIPTIONS
from agentless_mcp.util import fslimits, textsafe
from agentless_mcp.util.errors import AgentlessError, SecurityRefusal
from agentless_mcp.util.tokens import TokenCounter

logger = logging.getLogger(__name__)

SERVER_NAME = "agentless-mcp"

# The distribution name from pyproject, which the installed metadata is the only
# source of truth for. It equals SERVER_NAME today by coincidence, not by rule.
DISTRIBUTION_NAME = "agentless-mcp"

# Announced in the initialize handshake when the metadata is absent, which
# happens only in a source tree that was never installed. Not an empty string:
# a blank version reads as a server that answered rather than one that could
# not look itself up.
UNKNOWN_VERSION = "0+unknown"


def server_version() -> str:
    """The version the initialize handshake advertises for this server."""
    try:
        return distribution_version(DISTRIBUTION_NAME)
    except PackageNotFoundError:
        sys.stderr.write(
            f"agentless-mcp-server: no installed metadata for {DISTRIBUTION_NAME}, so the "
            f"initialize handshake will report version {UNKNOWN_VERSION}. This is a source "
            "tree that was never installed; install it to report a real version.\n"
        )
        return UNKNOWN_VERSION


# The transports this server will start. stdio is the default because a client
# that spawns the server as a child is the shape every registered client uses,
# and because it is the only one that needs no port, no binding decision and no
# second process to outlive the client.
# Annotated as literals because FastMCP types its own transport parameter as a
# Literal: a bare str here would type-check at the constant and fail at the call.
TRANSPORT_STDIO: Literal["stdio"] = "stdio"
TRANSPORT_HTTP: Literal["http"] = "http"
TRANSPORTS = (TRANSPORT_STDIO, TRANSPORT_HTTP)

# Where --transport http listens when the operator names no address. Loopback,
# and enforced rather than merely defaulted -- see _check_transport. The port is
# FastMCP's own default: deferring to the framework's documented number keeps
# this package from carrying one deployment's port allocation, and every
# deployment that cares passes --port anyway.
DEFAULT_HTTP_HOST = "127.0.0.1"
DEFAULT_HTTP_PORT = 8000

# A line range arrives as a two-element [start, end] list.
_RANGE_PAIR_LENGTH = 2

# How long a client gets to answer `roots/list`. It is a capability query
# answered from memory, so seconds is already generous; the number that
# matters is that there is one, because every tool call waits behind it.
_LIST_ROOTS_TIMEOUT_SECONDS = 2.0

# The tool surfaces this server can publish. v2 -- five intent-shaped tools --
# is the default; v1 keeps the original eleven for un-migrated operators, and
# both publishes the union for a transition window (find_referencing_symbols
# and capabilities are shared by the two surfaces, so the union is fourteen
# names, not sixteen). The flag is server-level: one process publishes one
# surface, whatever repositories it serves.
SURFACE_V1: Literal["v1"] = "v1"
SURFACE_V2: Literal["v2"] = "v2"
SURFACE_BOTH: Literal["both"] = "both"
SURFACES = (SURFACE_V1, SURFACE_V2, SURFACE_BOTH)
Surface = Literal["v1", "v2", "both"]

OPERATION_PATH = "path"
OPERATION_CYCLES = "cycles"
OPERATION_COMMUNITIES = "communities"
OPERATION_DIAGRAM = "diagram"

# The one parameter every tool shares. Its description is prompt data like
# the tool descriptions; pydantic carries it into the published schema, which
# is the only documentation an arbitrary client is guaranteed to read.
RepoRoot = Annotated[str | None, Field(description=PARAMETER_DESCRIPTIONS["repo_root"])]
MapFocus = Annotated[
    str | list[str] | None,
    Field(description=PARAMETER_DESCRIPTIONS["map_focus"]),
]
Granularity = Annotated[
    Literal["function", "file"] | None,
    Field(description=PARAMETER_DESCRIPTIONS["map_granularity"]),
]
NoCache = Annotated[bool, Field(description=PARAMETER_DESCRIPTIONS["no_cache"])]
TreePath = Annotated[str | None, Field(description=PARAMETER_DESCRIPTIONS["tree_path"])]
OverviewPaths = Annotated[list[str], Field(description=PARAMETER_DESCRIPTIONS["overview_paths"])]
Docstrings = Annotated[bool | None, Field(description=PARAMETER_DESCRIPTIONS["docstrings"])]
StableIds = Annotated[list[str], Field(description=PARAMETER_DESCRIPTIONS["stable_ids"])]
FilePath = Annotated[str, Field(description=PARAMETER_DESCRIPTIONS["file_path"])]
WholeFile = Annotated[bool, Field(description=PARAMETER_DESCRIPTIONS["whole_file"])]
FindName = Annotated[str, Field(description=PARAMETER_DESCRIPTIONS["find_name"])]
SymbolKindParameter = Annotated[
    SymbolKind | None,
    Field(description=PARAMETER_DESCRIPTIONS["symbol_kind"]),
]
ReferenceTarget = Annotated[
    str,
    Field(description=PARAMETER_DESCRIPTIONS["reference_target"]),
]
SharedCallers = Annotated[bool, Field(description=PARAMETER_DESCRIPTIONS["shared_callers"])]
ExplainTarget = Annotated[str, Field(description=PARAMETER_DESCRIPTIONS["explain_target"])]
StructureOperation = Annotated[
    Literal["path", "cycles", "communities", "diagram"],
    Field(description=PARAMETER_DESCRIPTIONS["structure_operation"]),
]
PathSource = Annotated[str, Field(description=PARAMETER_DESCRIPTIONS["path_source"])]
PathTarget = Annotated[str, Field(description=PARAMETER_DESCRIPTIONS["path_target"])]
IncludeUnique = Annotated[bool, Field(description=PARAMETER_DESCRIPTIONS["include_unique"])]
IncludeAmbiguous = Annotated[
    bool,
    Field(description=PARAMETER_DESCRIPTIONS["include_ambiguous"]),
]
DiagramFocus = Annotated[
    str | list[str] | None,
    Field(description=PARAMETER_DESCRIPTIONS["diagram_focus"]),
]
GroupByCommunities = Annotated[
    bool,
    Field(description=PARAMETER_DESCRIPTIONS["group_by_communities"]),
]
Locations = Annotated[list[str], Field(description=PARAMETER_DESCRIPTIONS["locations"])]

# Wire bounds. Every number a tool takes is bounded in the published schema,
# because that schema is the only refusal a model can read before it makes
# the call -- an out-of-range value comes back as a validation error naming
# the bound rather than as a nonsense answer from a service that sliced with
# it. Services keep their own checks; this is the gate in front of them.
MAX_LIMIT = 500
MAX_CONTEXT_LINES = 200
MAX_DIAGRAM_NODES = 500
MAX_RESOLUTION = 100.0

# ``limit`` is nullable on every tool that carries it because it is nullable
# on analyze_structure, whose default depends on the operation. A client that
# learns "limit may be null" from one schema and reuses it on another must not
# be refused for it (#13's coercion class); null reads as the tool's default.
ExpandLimit = Annotated[
    int | None,
    Field(ge=1, le=MAX_LIMIT, description=PARAMETER_DESCRIPTIONS["expand_limit"]),
]
FindLimit = Annotated[
    int | None,
    Field(ge=1, le=MAX_LIMIT, description=PARAMETER_DESCRIPTIONS["find_limit"]),
]
ReferenceLimit = Annotated[
    int | None,
    Field(ge=1, le=MAX_LIMIT, description=PARAMETER_DESCRIPTIONS["reference_limit"]),
]
ExplainLimit = Annotated[
    int | None,
    Field(ge=1, le=MAX_LIMIT, description=PARAMETER_DESCRIPTIONS["explain_limit"]),
]
StructureLimit = Annotated[
    int | None,
    Field(ge=1, le=MAX_LIMIT, description=PARAMETER_DESCRIPTIONS["structure_limit"]),
]
ContextLines = Annotated[
    int,
    Field(
        ge=0,
        le=MAX_CONTEXT_LINES,
        description=PARAMETER_DESCRIPTIONS["context_lines"],
    ),
]
Depth = Annotated[
    int,
    Field(
        ge=1,
        le=fslimits.DEFAULT_MAX_DEPTH,
        description=PARAMETER_DESCRIPTIONS["tree_depth"],
    ),
]
MaxEntries = Annotated[
    int,
    Field(
        ge=1,
        le=fslimits.DEFAULT_MAX_WALK_FILES,
        description=PARAMETER_DESCRIPTIONS["tree_max_entries"],
    ),
]
Budget = Annotated[
    int | None,
    Field(
        ge=projectconfig.MIN_BUDGET,
        le=projectconfig.MAX_BUDGET,
        description=PARAMETER_DESCRIPTIONS["map_budget"],
    ),
]
MaxFiles = Annotated[
    int | None,
    Field(
        ge=projectconfig.MIN_MAX_FILES,
        le=projectconfig.MAX_MAX_FILES,
        description=PARAMETER_DESCRIPTIONS["map_max_files"],
    ),
]
MaxNodes = Annotated[
    int,
    Field(
        ge=1,
        le=MAX_DIAGRAM_NODES,
        description=PARAMETER_DESCRIPTIONS["diagram_max_nodes"],
    ),
]
Resolution = Annotated[
    float | None,
    Field(
        gt=0,
        le=MAX_RESOLUTION,
        description=PARAMETER_DESCRIPTIONS["community_resolution"],
    ),
]

# One slice is [start, end] with 1-based lines, and the schema says so: a
# three-element or empty range that reached the handler would be dropped, and
# a read_slice with every range dropped renders the whole file.
LineRange = Annotated[list[Annotated[int, Field(ge=1)]], Field(min_length=2, max_length=2)]
LineRanges = Annotated[
    list[LineRange] | None,
    Field(min_length=1, description=PARAMETER_DESCRIPTIONS["slice_lines"]),
]

# The v2 surface's parameter types. operation is a plain string on purpose:
# requiredness there depends on the selected operation, which a flat schema
# cannot express, so the operation vocabulary and the per-operation parameter
# sets are enforced at runtime where the refusal can name the fix (see
# _checked_operation). Every per-operation parameter is therefore nullable at
# the schema layer -- None reads as "omitted" -- while its value shape and
# bounds stay identical to the v1 tool publishing the same name, which is what
# the shared-shape gate in the tests holds across surfaces.
OrientOperation = Annotated[str, Field(description=PARAMETER_DESCRIPTIONS["orient_operation"])]
SymbolsOperation = Annotated[str, Field(description=PARAMETER_DESCRIPTIONS["symbols_operation"])]
ReadOperation = Annotated[str, Field(description=PARAMETER_DESCRIPTIONS["read_operation"])]
OrientFocus = Annotated[
    str | list[str] | None,
    Field(description=PARAMETER_DESCRIPTIONS["orient_focus"]),
]
OrientLimit = Annotated[
    int | None,
    Field(ge=1, le=MAX_LIMIT, description=PARAMETER_DESCRIPTIONS["orient_limit"]),
]
SymbolsLimit = Annotated[
    int | None,
    Field(ge=1, le=MAX_LIMIT, description=PARAMETER_DESCRIPTIONS["symbols_limit"]),
]
ReadPath = Annotated[str | None, Field(description=PARAMETER_DESCRIPTIONS["read_path"])]
OptionalSource = Annotated[str | None, Field(description=PARAMETER_DESCRIPTIONS["path_source"])]
OptionalTarget = Annotated[str | None, Field(description=PARAMETER_DESCRIPTIONS["path_target"])]
OptionalIncludeUnique = Annotated[
    bool | None,
    Field(description=PARAMETER_DESCRIPTIONS["include_unique"]),
]
OptionalIncludeAmbiguous = Annotated[
    bool | None,
    Field(description=PARAMETER_DESCRIPTIONS["include_ambiguous"]),
]
OptionalGroupByCommunities = Annotated[
    bool | None,
    Field(description=PARAMETER_DESCRIPTIONS["group_by_communities"]),
]
OptionalMaxNodes = Annotated[
    int | None,
    Field(ge=1, le=MAX_DIAGRAM_NODES, description=PARAMETER_DESCRIPTIONS["diagram_max_nodes"]),
]
OptionalFindName = Annotated[str | None, Field(description=PARAMETER_DESCRIPTIONS["find_name"])]
OptionalOverviewPaths = Annotated[
    list[str] | None,
    Field(description=PARAMETER_DESCRIPTIONS["overview_paths"]),
]
OptionalStableIds = Annotated[
    list[str] | None,
    Field(description=PARAMETER_DESCRIPTIONS["stable_ids"]),
]
OptionalExplainTarget = Annotated[
    str | None,
    Field(description=PARAMETER_DESCRIPTIONS["explain_target"]),
]
OptionalFilePath = Annotated[str | None, Field(description=PARAMETER_DESCRIPTIONS["file_path"])]
OptionalLocations = Annotated[
    list[str] | None,
    Field(description=PARAMETER_DESCRIPTIONS["locations"]),
]
OptionalContextLines = Annotated[
    int | None,
    Field(ge=0, le=MAX_CONTEXT_LINES, description=PARAMETER_DESCRIPTIONS["context_lines"]),
]
OptionalWholeFile = Annotated[bool | None, Field(description=PARAMETER_DESCRIPTIONS["whole_file"])]
OptionalDepth = Annotated[
    int | None,
    Field(ge=1, le=fslimits.DEFAULT_MAX_DEPTH, description=PARAMETER_DESCRIPTIONS["tree_depth"]),
]
OptionalMaxEntries = Annotated[
    int | None,
    Field(
        ge=1,
        le=fslimits.DEFAULT_MAX_WALK_FILES,
        description=PARAMETER_DESCRIPTIONS["tree_max_entries"],
    ),
]


def _sole_selection(static: Sequence[Path], client_roots: Sequence[Path]) -> Path | None:
    """Return the one configured root the client's workspace identifies, if any.

    Static roots authorise; client roots only select among them. An advertised
    root names a configured root when one contains the other (a path contains
    itself): the workspace open inside a repository, or one directory above
    it. An advertised root that names none of them selects nothing -- it never
    authorises itself, so there is nothing there to select. Zero candidates or
    several is ordinary ambiguity; the caller refuses with the listing exactly
    as if nothing were advertised.
    """
    if len(static) == 1:
        return static[0]
    candidates = [
        root
        for root in static
        if any(
            root.is_relative_to(client) or client.is_relative_to(root) for client in client_roots
        )
    ]
    if len(candidates) == 1:
        return candidates[0]
    return None


def _root_lines(text: str) -> list[Path]:
    """The paths one roots file's text lists, blank and #-comment lines skipped."""
    # Whole-line comments only: a repository path may itself contain '#'.
    return [
        Path(stripped)
        for line in text.splitlines()
        if (stripped := line.strip()) and not stripped.startswith("#")
    ]


@dataclass
class RootsFile:
    """One ``--roots-from`` file and the roots it held when last read.

    The file is the operator-editable half of the allowlist: ``current()``
    re-reads it when its modification time changes, so an appended line enrols
    a repository on the next call and a removed line revokes one, without a
    restart. A file that stops being readable after startup is refused loudly
    on every call that needs it, never served from the stale copy: silence
    here would leave the operator editing a file the server had quietly
    stopped watching. Startup validation still happens in ``roots_file``, so
    a bad path fails the process before it ever answers a call.
    """

    path: Path
    roots: list[Path]
    stat_key: tuple[int, int]
    lock: threading.Lock = field(default_factory=threading.Lock, repr=False)

    def current(self) -> list[Path]:
        """This file's roots as of now, re-read if the file changed on disk.

        Change is keyed on (mtime_ns, size) rather than mtime alone: a
        rewrite landing within the filesystem's timestamp granularity almost
        always changes the byte count too, and both fields come from the one
        stat the freshness check already pays for.

        Held under a lock because the HTTP transport reaches this from
        concurrent request threads and the two fields are one fact: the roots
        and the stat key they were read at. Publishing them separately lets a
        second caller observe new roots against an old key, which costs a
        redundant re-read now and would cost correctness the moment anything
        else keyed on the pair. The same reasoning already guards the
        background index's registry.
        """
        with self.lock:
            try:
                stat_result = self.path.stat()
            except OSError as exc:
                message = MESSAGES.roots_file_unreadable.format(file=self.path, error=exc)
                raise SecurityRefusal(message) from exc
            key = (stat_result.st_mtime_ns, stat_result.st_size)
            if key == self.stat_key:
                return self.roots
            try:
                text = self.path.read_text(encoding="utf-8-sig")
            except (OSError, UnicodeDecodeError) as exc:
                message = MESSAGES.roots_file_unreadable.format(file=self.path, error=exc)
                raise SecurityRefusal(message) from exc
            self.roots = resolved_allowlist(_root_lines(text))
            self.stat_key = key
            return self.roots


@dataclass(frozen=True)
class StructureRequest:
    """One ``analyze_structure`` call: the operation and every operand.

    A value object rather than eight parameters threaded through the handler,
    because the wire signature is flat by necessity -- an MCP client reads the
    parameter list as the schema -- and the handler should not be.
    """

    operation: str
    source: str = ""
    target: str = ""
    include_unique: bool = False
    include_ambiguous: bool = False
    limit: int | None = None
    resolution: float | None = None
    focus: str = ""
    max_nodes: int = DEFAULT_DIAGRAM_NODES
    group_by_communities: bool = False


@dataclass(frozen=True)
class ServerServices:
    """The application services one server process needs, wired by bootstrap.

    The extractor is here as well as inside the services because opening a
    repository's tag cache is the adapter's job: the repository a call names
    and its ``no_cache`` argument are facts this layer holds and the services
    deliberately do not.
    """

    maps: MapService
    views: ViewService
    symbols: SymbolService
    graphs: GraphService
    counter: TokenCounter
    extractor: TreeSitterExtractor


class ToolHandlers:
    """The tool bodies, independent of FastMCP so they can be tested directly."""

    def __init__(
        self,
        roots: Sequence[Path],
        services: ServerServices,
        *,
        allow_client_roots: bool = False,
        roots_files: Sequence[RootsFile] = (),
        auto_index: bool = True,
    ) -> None:
        self._roots = tuple(roots)
        self._services = services
        self._allow_client_roots = allow_client_roots
        self._roots_files = tuple(roots_files)
        self._auto_index = auto_index

    @property
    def roots(self) -> tuple[Path, ...]:
        """The roots this server serves right now: the flags, then the files.

        Reading this re-reads any ``--roots-from`` file whose mtime changed,
        which is the property doing IO on purpose: every caller wants the
        allowlist as of this call, and a cached copy is exactly the stale
        answer the re-read exists to prevent. Deduplicated because a roots
        file overlapping a ``--root`` flag is an ordinary thing to write, and
        a repeated root would leave a server holding one repository refusing
        to default to it, as though it held two.
        """
        merged = [*self._roots, *chain.from_iterable(f.current() for f in self._roots_files)]
        return tuple(dict.fromkeys(merged))

    def _hinted(self, message: str) -> str:
        """Append the enrolment hint when an operator-editable roots file exists.

        The refusal is the one message an agent is guaranteed to read at the
        exact moment enrolment matters, so it carries the remediation: which
        file to append to, and that no restart is needed.
        """
        if not self._roots_files:
            return message
        listing = ", ".join(str(entry.path) for entry in self._roots_files)
        return f"{message} {MESSAGES.roots_file_hint.format(file=listing)}"

    def resolve(
        self,
        repo_root: str | None,
        client_roots: Sequence[Path] = (),
        *,
        no_cache: bool = False,
    ) -> RepoContext:
        """Authorise one call's repository and open the source it reads from.

        The allowlist is the configured roots and only those. What the client
        advertises is a selection hint, never an authorisation: a client that
        could widen this list would be the party deciding what the server
        serves, which is the operator's decision and is spelled ``--root``.

        ``--allow-client-roots`` restores the additive behaviour for operators
        who want it. It is a flag rather than the default because the two
        readings differ in who holds the decision, and only one of them can be
        the quiet one: a permissive default is a confinement boundary that
        stops confining without anyone typing anything.
        """
        allowed = list(self.roots)
        if self._allow_client_roots:
            allowed = list(dict.fromkeys([*allowed, *client_roots]))
        if not allowed:
            message = MESSAGES.server_no_roots
            raise SecurityRefusal(self._hinted(message))

        if repo_root is None or not repo_root.strip():
            selected = _sole_selection(allowed, client_roots)
            if selected is not None:
                return self._with_source(resolve_repo(selected, allowed), no_cache=no_cache)
            listing = ", ".join(str(path) for path in allowed)
            message = MESSAGES.server_root_required.format(roots=listing)
            raise SecurityRefusal(self._hinted(message))

        try:
            return self._with_source(resolve_repo(repo_root, allowed), no_cache=no_cache)
        except SecurityRefusal as refusal:
            raise SecurityRefusal(self._hinted(str(refusal))) from refusal

    def _with_source(self, ctx: RepoContext, *, no_cache: bool) -> RepoContext:
        """Open this call's symbol source: the tag cache, or on-demand parsing."""
        source = cache.open_source(
            ctx.root,
            self._services.extractor,
            tree_oid=ctx.tree_oid,
            no_cache=no_cache,
        )
        # First use of a repository is the auto-index trigger: per repo rather
        # than at startup because a server can hold many roots, and a stale
        # cache costs performance only -- this call is already served live
        # from ``source`` while the refresh lands for the ones after it.
        # A --no-cache call opts out of the cache and is taken at its word.
        if self._auto_index and not no_cache:
            cache.start_auto_index(
                ctx.root,
                self._services.extractor,
                tree_oid=ctx.tree_oid,
                head_sha=ctx.head_sha,
            )
        return replace(ctx, symbols=source)

    def repo_map(self, ctx: RepoContext, request: MapRequest) -> str:
        """Render a ranked, budgeted repository map.

        An omitted ``budget`` means auto-size unless the repository's
        ``.agentless-mcp.json`` names one; the map service resolves the other
        two settings, so both adapters get the same precedence.
        """
        if request.budget is None and ctx.config.map_budget is not None:
            request = replace(request, budget=ctx.config.map_budget)
        result = self._services.maps.build(ctx, request)
        return envelope.wrap(
            ctx,
            self._services.maps.render_text(result),
            counter=self._services.counter,
            truncation=envelope.Truncation(
                shown=result.included, total=result.candidates, unit="symbols"
            ),
        )

    def list_dir(
        self,
        ctx: RepoContext,
        path: str | None,
        depth: int,
        max_entries: int,
    ) -> str:
        """Render the gitignore-aware directory tree."""
        view = self._services.views.tree(
            ctx,
            path=path,
            depth=depth,
            max_entries=max_entries,
        )
        return self._wrap(ctx, view.text)

    def get_symbols_overview(
        self, ctx: RepoContext, paths: Sequence[str], *, docs: bool | None
    ) -> str:
        """Render each named file as signatures with bodies elided.

        Each file's block opens with the stable-id pattern for that file, so
        the ids expand_symbols takes can be read straight off the overview
        without churning the skeleton renderer line by line. The pattern is
        derived from the same helper that mints real ids, so a language whose
        prefix differs from its name stays truthful.
        """
        keep = projectconfig.resolve(docs, ctx.config.docstrings, False)
        views = self._services.views.skeleton(ctx, paths, docstrings=keep)
        blocks = []
        for view in views:
            if view.error:
                blocks.append(f"### {view.path}\n{view.error}")
                continue
            ids_line = MESSAGES.overview_stable_ids.format(
                pattern=stable_id(view.language, view.path, "<QualifiedName>")
            )
            blocks.append(f"### {view.path}\n{ids_line}\n{view.text}")
        return self._wrap(ctx, "\n".join(blocks))

    def expand_symbols(self, ctx: RepoContext, stable_ids: Sequence[str], limit: int) -> str:
        """Render bodies for the named stable ids, marking whatever was shortened."""
        result = self._services.symbols.expand_symbols(ctx, list(stable_ids), limit=limit)
        return self._wrap(ctx, render_expansion(result))

    def read_slice(
        self,
        ctx: RepoContext,
        path: str,
        intervals: Sequence[tuple[int, int]],
        context_lines: int,
    ) -> str:
        """Render numbered lines with sticky-scroll scope headers."""
        view = self._services.views.read_slice(
            ctx, path, intervals=intervals, context=context_lines
        )
        return self._wrap(ctx, view.text or view.error)

    def find_symbol(self, ctx: RepoContext, name: str, kind: str | None, limit: int) -> str:
        """Render incident cards for symbols matching ``name``."""
        result = self._services.symbols.find_symbol(ctx, name, kind=kind, limit=limit)
        return self._wrap(ctx, render_find(result))

    def find_referencing_symbols(
        self,
        ctx: RepoContext,
        target: str,
        limit: int,
        *,
        shared_callers: bool,
    ) -> str:
        """Render fan-in for ``target``, grouped by file."""
        result = self._services.symbols.find_referencing_symbols(
            ctx, target, limit=limit, shared_callers=shared_callers
        )
        body = (
            render.render_shared_callers(result.shared, target)
            if shared_callers
            else render.render_ref_groups(result.groups, target)
        )
        return self._wrap(ctx, body)

    def explain_symbol(self, ctx: RepoContext, target: str, limit: int) -> str:
        """Render one symbol's definition site with its tiered fan-out and fan-in."""
        result = self._services.graphs.explain(ctx, target, limit=limit)
        return self._wrap(ctx, render.render_explanation(result))

    def analyze_structure(self, ctx: RepoContext, request: StructureRequest) -> str:
        """Answer one structural question about the repository as a whole.

        Four questions behind one tool, because they are one question shape --
        "how is this repository put together" -- and a client picking between
        eleven tools picks better than one picking between fourteen. Over the
        v1 wire the published enum on ``operation`` rejects an unknown value
        before this branch can; it stays as the backstop for direct handler
        callers, and it keeps the dispatch and the message from disagreeing
        about which operations exist. The v2 ``orient`` surface publishes no
        enum and validates the operation itself before routing here.
        """
        handler = _OPERATIONS.get(request.operation)
        if handler is None:
            listed = ", ".join(sorted(_OPERATIONS))
            message = MESSAGES.unknown_operation.format(
                tool="analyze_structure", operation=request.operation, operations=listed
            )
            raise AgentlessError(message)
        return self._wrap(ctx, handler(self._services.graphs, ctx, request))

    def resolve_locations(
        self,
        ctx: RepoContext,
        path: str,
        locs: Sequence[str],
        context_lines: int,
    ) -> str:
        """Resolve location strings to stable ids and merged intervals."""
        view = self._services.views.resolve_locations(ctx, path, locs, context=context_lines)
        lines = [f"file: {view.path}"]
        lines.extend(f"matched: {stable}" for stable in view.resolution.stable_ids)
        lines.append(
            "intervals: "
            + (", ".join(f"{start}-{end}" for start, end in view.resolution.intervals) or "none")
        )
        lines.extend(
            f"unrecognized: {entry.loc} -- {entry.reason}" for entry in view.resolution.unrecognized
        )
        if view.text:
            lines.extend(["", view.text.rstrip("\n")])
        return self._wrap(ctx, "\n".join(lines) + "\n")

    def capabilities(self, ctx: RepoContext, client_roots: Sequence[Path] = ()) -> str:
        """Report the complete application-owned capability contract."""
        report = build_capability_report(
            ctx,
            self._services.extractor,
            configured_roots=self.roots,
            client_roots=client_roots,
        )
        return self._wrap(ctx, render_capability_report(report))

    def _wrap(self, ctx: RepoContext, body: str) -> str:
        """Put the receipt and banner around one tool's answer."""
        return envelope.wrap(ctx, body, counter=self._services.counter)


def _operation_path(graphs: GraphService, ctx: RepoContext, request: StructureRequest) -> str:
    """Render the shortest resolved path between two named endpoints."""
    if not request.source.strip() or not request.target.strip():
        message = MESSAGES.path_needs_endpoints
        raise AgentlessError(message)
    trace = graphs.path(
        ctx,
        request.source,
        request.target,
        PathOptions(
            include_unique=request.include_unique,
            include_ambiguous=request.include_ambiguous,
        ),
    )
    return render.render_path(trace)


def _operation_cycles(graphs: GraphService, ctx: RepoContext, request: StructureRequest) -> str:
    """Render every module-level import cycle."""
    limit = _or_default(request.limit, DEFAULT_CYCLE_LIMIT)
    return render.render_cycles(graphs.cycles(ctx, limit=limit))


def _operation_communities(
    graphs: GraphService, ctx: RepoContext, request: StructureRequest
) -> str:
    """Render the file communities, largest first."""
    report = graphs.communities(
        ctx,
        resolution=request.resolution,
        limit=_or_default(request.limit, DEFAULT_COMMUNITY_LIMIT),
    )
    return render.render_communities(report)


def _or_default(value: int | None, fallback: int) -> int:
    """Return the caller's bound, or this view's own when they named none.

    One ``limit`` on the wire serves two listings, and the two views own
    different defaults. Resolving per operation is what stops the cycle
    listing's bound from quietly becoming the community listing's.
    """
    return fallback if value is None else value


def _operation_diagram(graphs: GraphService, ctx: RepoContext, request: StructureRequest) -> str:
    """Render the module graph as fenced mermaid text."""
    view = graphs.diagram(
        ctx,
        focus=request.focus or None,
        max_nodes=request.max_nodes,
        group_by_communities=request.group_by_communities,
        resolution=request.resolution,
    )
    return render.render_diagram(view)


# The operations `analyze_structure` accepts, and what each one runs. A table
# rather than a chain of branches so that the tool's own error message and its
# dispatch cannot disagree about which operations exist.
_OPERATIONS: dict[str, Callable[[GraphService, RepoContext, StructureRequest], str]] = {
    OPERATION_PATH: _operation_path,
    OPERATION_CYCLES: _operation_cycles,
    OPERATION_COMMUNITIES: _operation_communities,
    OPERATION_DIAGRAM: _operation_diagram,
}


async def effective_client_roots(context: Context) -> list[Path]:
    """Return the directories the connected client advertises as its workspace.

    A selection hint, never an authorisation: all these roots can do is pick
    out one of the configured roots for a call that omitted ``repo_root``.

    Asking is an out-of-process round trip to the client, on the critical path
    of every tool, so it is bounded: a capability query that has not answered
    in seconds is not going to, and a client that never answers must not hang
    the tool that asked on its behalf. The failures converted here --
    unimplemented, timed out, malformed payload, socket error -- all mean the
    same thing, "no advertised roots", and leave the static roots standing. A
    transport torn down mid-call is not one of them: anyio raises its own
    stream errors, which derive from Exception rather than OSError, and a call
    whose transport is gone has nowhere to return an answer anyway.
    """
    try:
        roots = await asyncio.wait_for(context.list_roots(), _LIST_ROOTS_TIMEOUT_SECONDS)
    except McpError as exc:
        logger.debug("client does not advertise MCP roots (%s); using --root only", exc)
        return []
    except (asyncio.TimeoutError, ValidationError, OSError) as exc:
        logger.warning("MCP roots/list failed (%s); using --root only", exc)
        return []

    advertised = (_client_root(root.uri) for root in roots)
    return [path for path in advertised if path is not None]


def _client_root(uri: object) -> Path | None:
    """Parse one advertised root URI into a directory, or refuse it and say so.

    This is the one place foreign data becomes a path in this adapter, so it
    either yields an existing local directory or nothing. ``urlparse`` alone
    does not: ``file://`` parses to an empty path that ``Path.resolve`` turns
    into the server's own working directory, and ``file://host/etc`` parses to
    a remote authority that is then silently dropped. Both are refusals here,
    logged with the URI that caused them.
    """
    try:
        return _resolved_client_root(uri)
    except (OSError, ValueError) as exc:
        logger.warning("ignoring MCP root %s: %s", uri, exc)
        return None


def _resolved_client_root(uri: object) -> Path:
    """Convert one valid local file URI into an existing resolved directory."""
    parsed = urlparse(str(uri))
    if parsed.scheme != "file":
        message = "not a file: URI"
        raise ValueError(message)
    if parsed.netloc not in ("", "localhost"):
        message = "names a remote authority"
        raise ValueError(message)
    if not parsed.path:
        message = "has no path"
        raise ValueError(message)

    decoded = unquote(parsed.path)
    # Checked on the DECODED form, before the path is built: `%0A` survives
    # percent-decoding as a real newline, and a root carrying one reaches the
    # receipt, which is the tool's own framing above the trust banner. Refused
    # rather than escaped -- at an entry point a control character in a
    # directory name is invalid input, and rejecting says so; escaping here
    # would double up against the escape the receipt already applies.
    if textsafe.has_line_break(decoded):
        message = "path contains a control character"
        raise ValueError(message)

    path = Path(decoded)
    if not path.is_absolute():
        message = "path is not absolute"
        raise ValueError(message)

    resolved = path.resolve()
    if not resolved.is_dir():
        message = "not an existing directory"
        raise ValueError(message)
    return resolved


def _intervals(
    ranges: Sequence[Sequence[int]], *, tool: str = "read_slice"
) -> list[tuple[int, int]]:
    """Parse the wire's line ranges into intervals, or refuse the whole call.

    Refused rather than filtered: the slice renders the whole file when no
    interval survives, so dropping a malformed range answers a bounded
    request with the substitute content this tool promises never to return.
    The schema rejects a range that is not two 1-based line numbers before it
    reaches here; the shape it cannot express is an end before its start.
    ``tool`` names the refusing surface -- ``read_slice`` on v1, the ``read``
    tool's slice operation on v2 -- so the message blames the call the agent
    actually made.
    """
    intervals: list[tuple[int, int]] = []
    for pair in ranges:
        if len(pair) != _RANGE_PAIR_LENGTH or pair[1] < pair[0]:
            listed = ", ".join(str(number) for number in pair)
            message = (
                f"{tool} range [{listed}] is not a line range: each one is "
                "[start, end], 1-based and inclusive, with end at or after start."
            )
            raise AgentlessError(message)
        intervals.append((pair[0], pair[1]))
    return intervals


def _sole_focus(focus: str | list[str] | None) -> str:
    """Reduce a diagram focus to the single seed the operation supports.

    ``repo_map.focus`` is a list, so a client bridging the two tools can turn
    a correct string into a one-element list here; rejecting that shape reads
    to the caller as its own typing mistake. A diagram has one centre, so a
    list is accepted and only its first entry is used -- the published
    parameter description says so.
    """
    if focus is None:
        return ""
    if isinstance(focus, str):
        return focus
    return focus[0] if focus else ""


def _focus_entries(focus: str | list[str] | None) -> tuple[str, ...]:
    """Normalize a map focus to the seed tuple ``MapRequest`` carries.

    The mirror of ``_sole_focus``: ``analyze_structure.focus`` takes a bare
    string, so a client bridging the two tools can send one here, and it
    reads as a one-element list (#13's coercion class). An empty string is
    no focus, exactly as an empty list is.
    """
    if focus is None:
        return ()
    if isinstance(focus, str):
        return (focus,) if focus else ()
    return tuple(focus)


def _slice_intervals(
    ranges: Sequence[Sequence[int]] | None,
    whole_file: bool,
    *,
    tool: str = "read_slice",
) -> list[tuple[int, int]]:
    """Parse an explicit bounded slice or an explicit whole-file request."""
    if ranges is not None and whole_file:
        message = f"{tool} accepts lines or whole_file=true, not both"
        raise AgentlessError(message)
    if ranges is None and not whole_file:
        message = f"{tool} requires non-empty lines or explicit whole_file=true"
        raise AgentlessError(message)
    return [] if ranges is None else _intervals(ranges, tool=tool)


# Published as MCP ``_meta`` on every v2-surface tool: clients that defer
# tool schemas behind a search step (Claude Code tool search) load these at
# session start instead. v1 tools never carry it; that surface is kept only
# for migration and should not spend always-loaded schema budget.
ALWAYS_LOAD_META = {"anthropic/alwaysLoad": True}

# What a registrar needs to open one call's repository: the context_for
# closure build_server makes over its handlers.
RepoContextFactory = Callable[..., AbstractAsyncContextManager[RepoContext]]


# The exceptions whose own text was written for a caller to read: this
# package's refusals, and the framework's own -- a wire-schema rejection
# naming the values a parameter accepts is FastMCP speaking, and that is
# exactly the message an agent needs to correct its call.
_DELIBERATE_ERRORS = (AgentlessError, FastMCPError, ValidationError)

_UNPLANNED_ERROR_MESSAGE = (
    "the tool failed for a reason it does not handle; the server log has the detail. "
    "This is a defect in agentless-mcp, not something the call can be corrected to avoid."
)


class _DeliberateErrorsOnly(Middleware):
    """Let this package's own refusals through; replace anything else.

    With ``mask_error_details=False`` an unhandled exception's text is what
    reaches the client, and the exceptions this package does not plan for
    carry local detail: a ``sqlite3`` failure names the absolute path of the
    tag cache, an ``OSError`` names the file it could not open. Neither is
    something to hand to a caller across a transport (CWE-209).

    The split is by *authorship*, not by severity. A message this package or
    FastMCP wrote is a message a caller can act on -- it names the operation,
    what it accepts, what it requires. Everything else is a defect, and a
    defect's own words were written for whoever reads the log. So the log gets
    them, in full and with the traceback, and the caller gets a sentence that
    says a defect happened.

    The test is on ``__cause__`` rather than on the exception itself because
    FastMCP wraps whatever a tool raises into a ``ToolError`` before any
    middleware sees it. A ``ToolError`` with no cause is the framework
    speaking for itself, which is deliberate too.
    """

    async def on_call_tool(
        self,
        context: MiddlewareContext[mcp_types.CallToolRequestParams],
        call_next: CallNext[mcp_types.CallToolRequestParams, ToolResult],
    ) -> ToolResult:
        """Run one tool call, converting an unplanned failure at the boundary."""
        try:
            return await call_next(context)
        except _DELIBERATE_ERRORS as error:
            cause = error.__cause__
            if cause is None or isinstance(cause, _DELIBERATE_ERRORS):
                raise
            logger.exception("unhandled error in an agentless-mcp tool call")
            raise ToolError(_UNPLANNED_ERROR_MESSAGE) from None
        except Exception:
            logger.exception("unhandled error in an agentless-mcp tool call")
            raise ToolError(_UNPLANNED_ERROR_MESSAGE) from None


def build_server(handlers: ToolHandlers, surface: Surface = SURFACE_V2) -> FastMCP[None]:
    """Register the selected tool surface on a FastMCP server and return it.

    Each tool's wire description is passed explicitly from
    ``agentless_mcp.prompts``: the text a model reads is prompt data, revised
    on its own terms, and FastMCP would otherwise publish whatever the
    docstring happened to say. The docstrings in the registrars are code
    documentation. ``find_referencing_symbols`` and ``capabilities`` belong to
    every surface: the expensive fan-in call keeps its own decision point and
    cost warning on v2 deliberately, and the capability report is the same
    contract either way.
    """
    # Without an explicit version FastMCP advertises its own in the initialize
    # handshake, which tells a client the version of the framework rather than
    # of this server.
    # `mask_error_details` is pinned rather than left to FastMCP's default,
    # which reads FASTMCP_MASK_ERROR_DETAILS from the environment. Measured:
    # 243 of these tests pass with the variable unset and 22 fail with it set
    # to true, because a masked error replaces this package's own refusal
    # text -- which names the operation, what it accepts and what it requires
    # -- with a generic message. The refusal wording is the contract an agent
    # reads to correct its own call, so whether it survives must not depend
    # on an operator's shell.
    #
    # False is safe here because every message that reaches this boundary is
    # written by this package: `_safe_tool_error` below turns anything else
    # into a deliberately worded error before FastMCP sees it.
    mcp: FastMCP[None] = FastMCP(SERVER_NAME, version=server_version(), mask_error_details=False)
    mcp.add_middleware(_DeliberateErrorsOnly())

    @asynccontextmanager
    async def context_for(
        context: Context,
        repo_root: str | None,
        *,
        no_cache: bool = False,
    ) -> AsyncIterator[RepoContext]:
        roots = await effective_client_roots(context)
        ctx = handlers.resolve(repo_root, roots, no_cache=no_cache)
        try:
            yield ctx
        finally:
            ctx.close()

    _register_shared(mcp, handlers, context_for)
    if surface in (SURFACE_V1, SURFACE_BOTH):
        _register_v1(mcp, handlers, context_for)
    if surface in (SURFACE_V2, SURFACE_BOTH):
        _register_v2(mcp, handlers, context_for)
    return mcp


def _register_v1(
    mcp: FastMCP[None], handlers: ToolHandlers, context_for: RepoContextFactory
) -> None:
    """Register the v1-only tools: one tool per question, nine of them."""

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["repo_map"],
        annotations=read_only("Repository map"),
    )
    async def repo_map(
        context: Context,
        repo_root: RepoRoot = None,
        focus: MapFocus = None,
        budget: Budget = None,
        max_files: MaxFiles = None,
        granularity: Granularity = None,
        no_cache: NoCache = False,
    ) -> str:
        """Rank the repository's files and render the symbols that fit a budget."""
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            return handlers.repo_map(
                ctx,
                MapRequest(
                    focus=_focus_entries(focus),
                    budget=budget,
                    max_files=max_files,
                    granularity=granularity,
                ),
            )

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["list_dir"],
        annotations=read_only("Directory tree"),
    )
    async def list_dir(
        context: Context,
        repo_root: RepoRoot = None,
        path: TreePath = None,
        depth: Depth = DEFAULT_RENDER_DEPTH,
        max_entries: MaxEntries = DEFAULT_MAX_ENTRIES,
    ) -> str:
        """List the repository's files, honouring gitignore."""
        async with context_for(context, repo_root) as ctx:
            return handlers.list_dir(ctx, path, depth, max_entries)

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["get_symbols_overview"],
        annotations=read_only("Symbols overview"),
    )
    async def get_symbols_overview(
        context: Context,
        paths: OverviewPaths,
        repo_root: RepoRoot = None,
        docstrings: Docstrings = None,
        no_cache: NoCache = False,
    ) -> str:
        """Render the named files as signatures with their bodies elided."""
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            return handlers.get_symbols_overview(ctx, paths, docs=docstrings)

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["expand_symbols"],
        annotations=read_only("Expand symbols"),
    )
    async def expand_symbols(
        context: Context,
        stable_ids: StableIds,
        repo_root: RepoRoot = None,
        limit: ExpandLimit = None,
        no_cache: NoCache = False,
    ) -> str:
        """Return the full body of each named symbol, line-numbered."""
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            return handlers.expand_symbols(
                ctx, stable_ids, _or_default(limit, DEFAULT_EXPAND_LIMIT)
            )

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["read_slice"],
        annotations=read_only("Read slice"),
    )
    async def read_slice(
        context: Context,
        path: FilePath,
        repo_root: RepoRoot = None,
        lines: LineRanges = None,
        context_lines: ContextLines = DEFAULT_CONTEXT_LINES,
        whole_file: WholeFile = False,
    ) -> str:
        """Return numbered lines for the given 1-based inclusive ranges."""
        async with context_for(context, repo_root) as ctx:
            return handlers.read_slice(
                ctx,
                path,
                _slice_intervals(lines, whole_file),
                context_lines,
            )

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["find_symbol"],
        annotations=read_only("Find symbol"),
    )
    async def find_symbol(
        context: Context,
        name: FindName,
        repo_root: RepoRoot = None,
        kind: SymbolKindParameter = None,
        limit: FindLimit = None,
        no_cache: NoCache = False,
    ) -> str:
        """Find symbols by substring or qualified name."""
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            return handlers.find_symbol(ctx, name, kind, _or_default(limit, DEFAULT_FIND_LIMIT))

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["explain_symbol"],
        annotations=read_only("Explain symbol"),
    )
    async def explain_symbol(
        context: Context,
        target: ExplainTarget,
        repo_root: RepoRoot = None,
        limit: ExplainLimit = None,
        no_cache: NoCache = False,
    ) -> str:
        """Render one symbol's definition site, tiered fan-out, fan-in and imports."""
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            return handlers.explain_symbol(ctx, target, _or_default(limit, DEFAULT_EXPLAIN_LIMIT))

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["analyze_structure"],
        annotations=read_only("Analyze structure"),
    )
    async def analyze_structure(
        context: Context,
        operation: StructureOperation,
        repo_root: RepoRoot = None,
        source: PathSource = "",
        target: PathTarget = "",
        include_unique: IncludeUnique = False,
        include_ambiguous: IncludeAmbiguous = False,
        limit: StructureLimit = None,
        resolution: Resolution = None,
        focus: DiagramFocus = None,
        max_nodes: MaxNodes = DEFAULT_DIAGRAM_NODES,
        group_by_communities: GroupByCommunities = False,
        no_cache: NoCache = False,
    ) -> str:
        """Answer one whole-repository structural question: path, cycles, communities, diagram."""
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            return handlers.analyze_structure(
                ctx,
                StructureRequest(
                    operation=operation,
                    source=source,
                    target=target,
                    include_unique=include_unique,
                    include_ambiguous=include_ambiguous,
                    limit=limit,
                    resolution=resolution,
                    focus=_sole_focus(focus),
                    max_nodes=max_nodes,
                    group_by_communities=group_by_communities,
                ),
            )

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["resolve_locations"],
        annotations=read_only("Resolve locations"),
    )
    async def resolve_locations(
        context: Context,
        path: FilePath,
        locs: Locations,
        repo_root: RepoRoot = None,
        context_lines: ContextLines = DEFAULT_CONTEXT_LINES,
    ) -> str:
        """Turn class:/function:/line: strings into stable ids and intervals."""
        async with context_for(context, repo_root) as ctx:
            return handlers.resolve_locations(ctx, path, locs, context_lines)


def _register_shared(
    mcp: FastMCP[None], handlers: ToolHandlers, context_for: RepoContextFactory
) -> None:
    """Register the tools every surface publishes, unchanged between them.

    ``find_referencing_symbols`` stays its own tool on v2 deliberately: the
    expensive fan-in call keeps its own decision point, name and cost warning
    rather than hiding behind an operation value. ``capabilities`` is the same
    contract on both surfaces.
    """

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["find_referencing_symbols"],
        annotations=read_only("Find referencing symbols"),
        meta=ALWAYS_LOAD_META,
    )
    async def find_referencing_symbols(
        context: Context,
        target: ReferenceTarget,
        repo_root: RepoRoot = None,
        limit: ReferenceLimit = None,
        shared_callers: SharedCallers = False,
    ) -> str:
        """Find the symbols that reference a target, grouped by file."""
        async with context_for(context, repo_root) as ctx:
            return handlers.find_referencing_symbols(
                ctx,
                target,
                _or_default(limit, DEFAULT_REFS_LIMIT),
                shared_callers=shared_callers,
            )

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["capabilities"],
        annotations=read_only("Capabilities"),
        meta=ALWAYS_LOAD_META,
    )
    async def capabilities(context: Context, repo_root: RepoRoot = None) -> str:
        """Report loaded grammars, cache state and the bounds in force."""
        roots = await effective_client_roots(context)
        ctx = handlers.resolve(repo_root, roots)
        try:
            return handlers.capabilities(ctx, roots)
        finally:
            ctx.close()


@dataclass(frozen=True)
class OperationSpec:
    """One v2 operation: the parameters it accepts and the subset it requires.

    The universal parameters -- repo_root, operation, and no_cache where the
    tool carries it -- belong to every operation and are not listed.
    """

    accepted: tuple[str, ...]
    required: tuple[str, ...] = ()


# The v2 operation tables. Tables rather than branch chains for the same
# reason as _OPERATIONS: the rejection message and the dispatch must never
# disagree about which operations exist or what each one takes.
OPERATION_MAP = "map"
OPERATION_FIND = "find"
OPERATION_OVERVIEW = "overview"
OPERATION_EXPAND = "expand"
OPERATION_EXPLAIN = "explain"
OPERATION_LOCATE = "locate"
OPERATION_SLICE = "slice"
OPERATION_DIR = "dir"

ORIENT_OPERATIONS: dict[str, OperationSpec] = {
    OPERATION_MAP: OperationSpec(accepted=("focus", "budget", "limit", "granularity")),
    OPERATION_COMMUNITIES: OperationSpec(accepted=("resolution", "limit")),
    OPERATION_CYCLES: OperationSpec(accepted=("limit",)),
    OPERATION_DIAGRAM: OperationSpec(
        accepted=("focus", "max_nodes", "group_by_communities", "resolution")
    ),
    OPERATION_PATH: OperationSpec(
        accepted=("source", "target", "include_unique", "include_ambiguous"),
        required=("source", "target"),
    ),
}

SYMBOLS_OPERATIONS: dict[str, OperationSpec] = {
    OPERATION_FIND: OperationSpec(accepted=("name", "kind", "limit"), required=("name",)),
    OPERATION_OVERVIEW: OperationSpec(accepted=("paths", "docstrings"), required=("paths",)),
    OPERATION_EXPAND: OperationSpec(accepted=("stable_ids", "limit"), required=("stable_ids",)),
    OPERATION_EXPLAIN: OperationSpec(accepted=("target", "limit"), required=("target",)),
    OPERATION_LOCATE: OperationSpec(
        accepted=("path", "locations", "context_lines"), required=("path", "locations")
    ),
}

READ_OPERATIONS: dict[str, OperationSpec] = {
    OPERATION_SLICE: OperationSpec(
        accepted=("path", "lines", "context_lines", "whole_file"), required=("path",)
    ),
    OPERATION_DIR: OperationSpec(accepted=("path", "depth", "max_entries")),
}


def _omitted(value: object) -> bool:
    """Was this per-operation parameter left unset?

    One definition for both halves of the check. A blank string counts, and so
    does ``False`` on a flag: neither carries an instruction, and a client that
    fills every declared optional with a zero value -- the ordinary shape of a
    generated call -- is saying nothing by them. Refusing such a value as a
    stray parameter refuses a call that asked for nothing unusual, and for a
    flag whose v1 counterpart defaulted to ``False`` it refuses the default.
    """
    if value is None or value is False:
        return True
    return isinstance(value, str) and not value.strip()


def _checked_map_limit(operation: str, limit: int | None) -> None:
    """Hold ``orient``'s map operation to the file cap its v1 counterpart publishes.

    ``limit`` is one parameter serving three operations, and they do not share a
    ceiling: communities and cycles are listings bounded by :data:`MAX_LIMIT`,
    while map's cap is the repository-map bound ``repo_map`` publishes as
    ``max_files``. Keeping one shape on the wire is what the shared-parameter
    pass bought; the difference between the two ceilings is a contract, so it is
    enforced here rather than by splitting the parameter into two schemas that
    a client would then have to tell apart.
    """
    if operation != OPERATION_MAP or limit is None:
        return
    if not projectconfig.MIN_MAX_FILES <= limit <= projectconfig.MAX_MAX_FILES:
        message = MESSAGES.map_limit_out_of_range.format(
            limit=limit,
            minimum=projectconfig.MIN_MAX_FILES,
            maximum=projectconfig.MAX_MAX_FILES,
        )
        raise AgentlessError(message)


def _checked_operation(
    tool: str,
    operation: str,
    specs: Mapping[str, OperationSpec],
    provided: Mapping[str, object],
) -> None:
    """Validate one v2 call's operation and parameter set, or refuse it by name.

    The v2 tools publish ``operation`` as a plain string -- no wire enum -- so
    this is the one gate, and every refusal names the fix: an unknown
    operation is answered with the valid list, a parameter foreign to the
    selected operation and a missing required parameter are each answered with
    the operation, what it accepts and what it requires. Never a schema
    validation dump. ``provided`` maps every per-operation parameter to its
    wire value, where ``None`` reads as omitted.
    """
    spec = specs.get(operation)
    if spec is None:
        message = MESSAGES.unknown_operation.format(
            tool=tool, operation=operation, operations=", ".join(sorted(specs))
        )
        raise AgentlessError(message)
    accepted = ", ".join(spec.accepted)
    required = ", ".join(spec.required) or "none"
    stray = sorted(
        name
        for name, value in provided.items()
        if not _omitted(value) and name not in spec.accepted
    )
    if stray:
        message = MESSAGES.op_rejects_parameters.format(
            tool=tool,
            operation=operation,
            stray=", ".join(stray),
            accepted=accepted,
            required=required,
        )
        raise AgentlessError(message)
    missing = [name for name in spec.required if _omitted(provided.get(name))]
    if missing:
        message = MESSAGES.op_requires_parameters.format(
            tool=tool,
            operation=operation,
            missing=", ".join(missing),
            accepted=accepted,
            required=required,
        )
        raise AgentlessError(message)


def _register_v2(
    mcp: FastMCP[None], handlers: ToolHandlers, context_for: RepoContextFactory
) -> None:
    """Register the v2 surface: three consolidated intent-shaped tools.

    Adapter-layer routing only: every operation reaches exactly the handler
    its v1 counterpart tool calls, with the same defaults, so a v2 answer is
    byte-identical to its v1 counterpart's. Each tool validates the operation
    and its parameter set through ``_checked_operation`` before resolving the
    repository, so a malformed call is refused without opening anything.
    """

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["orient"],
        annotations=read_only("Orient"),
        meta=ALWAYS_LOAD_META,
    )
    async def orient(
        context: Context,
        operation: OrientOperation,
        repo_root: RepoRoot = None,
        focus: OrientFocus = None,
        budget: Budget = None,
        granularity: Granularity = None,
        resolution: Resolution = None,
        limit: OrientLimit = None,
        source: OptionalSource = None,
        target: OptionalTarget = None,
        include_unique: OptionalIncludeUnique = None,
        include_ambiguous: OptionalIncludeAmbiguous = None,
        max_nodes: OptionalMaxNodes = None,
        group_by_communities: OptionalGroupByCommunities = None,
        no_cache: NoCache = False,
    ) -> str:
        """Route one orientation operation to the map or graph services."""
        _checked_operation(
            "orient",
            operation,
            ORIENT_OPERATIONS,
            {
                "focus": focus,
                "budget": budget,
                "granularity": granularity,
                "resolution": resolution,
                "limit": limit,
                "source": source,
                "target": target,
                "include_unique": include_unique,
                "include_ambiguous": include_ambiguous,
                "max_nodes": max_nodes,
                "group_by_communities": group_by_communities,
            },
        )
        _checked_map_limit(operation, limit)
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            if operation == OPERATION_MAP:
                return handlers.repo_map(
                    ctx,
                    MapRequest(
                        focus=_focus_entries(focus),
                        budget=budget,
                        max_files=limit,
                        granularity=granularity,
                    ),
                )
            return handlers.analyze_structure(
                ctx,
                StructureRequest(
                    operation=operation,
                    source=source or "",
                    target=target or "",
                    include_unique=bool(include_unique),
                    include_ambiguous=bool(include_ambiguous),
                    limit=limit,
                    resolution=resolution,
                    focus=_sole_focus(focus),
                    max_nodes=_or_default(max_nodes, DEFAULT_DIAGRAM_NODES),
                    group_by_communities=bool(group_by_communities),
                ),
            )

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["symbols"],
        annotations=read_only("Symbols"),
        meta=ALWAYS_LOAD_META,
    )
    async def symbols(
        context: Context,
        operation: SymbolsOperation,
        repo_root: RepoRoot = None,
        name: OptionalFindName = None,
        kind: SymbolKindParameter = None,
        limit: SymbolsLimit = None,
        paths: OptionalOverviewPaths = None,
        docstrings: Docstrings = None,
        stable_ids: OptionalStableIds = None,
        target: OptionalExplainTarget = None,
        path: OptionalFilePath = None,
        locations: OptionalLocations = None,
        context_lines: OptionalContextLines = None,
        no_cache: NoCache = False,
    ) -> str:
        """Route one symbol operation to the symbol, view, or graph services."""
        _checked_operation(
            "symbols",
            operation,
            SYMBOLS_OPERATIONS,
            {
                "name": name,
                "kind": kind,
                "limit": limit,
                "paths": paths,
                "docstrings": docstrings,
                "stable_ids": stable_ids,
                "target": target,
                "path": path,
                "locations": locations,
                "context_lines": context_lines,
            },
        )
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            if operation == OPERATION_FIND:
                return handlers.find_symbol(
                    ctx, name or "", kind, _or_default(limit, DEFAULT_FIND_LIMIT)
                )
            if operation == OPERATION_OVERVIEW:
                return handlers.get_symbols_overview(ctx, paths or [], docs=docstrings)
            if operation == OPERATION_EXPAND:
                return handlers.expand_symbols(
                    ctx, stable_ids or [], _or_default(limit, DEFAULT_EXPAND_LIMIT)
                )
            if operation == OPERATION_EXPLAIN:
                return handlers.explain_symbol(
                    ctx, target or "", _or_default(limit, DEFAULT_EXPLAIN_LIMIT)
                )
            # The remaining table entry is OPERATION_LOCATE. _checked_operation
            # has already refused anything outside SYMBOLS_OPERATIONS, and the
            # parity table pairs every table entry with its CLI rendering, so an
            # operation added to the table without a branch fails there rather
            # than silently landing on this arm.
            return handlers.resolve_locations(
                ctx,
                path or "",
                locations or [],
                _or_default(context_lines, DEFAULT_CONTEXT_LINES),
            )

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["read"],
        annotations=read_only("Read"),
        meta=ALWAYS_LOAD_META,
    )
    async def read(
        context: Context,
        operation: ReadOperation,
        repo_root: RepoRoot = None,
        path: ReadPath = None,
        lines: LineRanges = None,
        context_lines: OptionalContextLines = None,
        whole_file: OptionalWholeFile = None,
        depth: OptionalDepth = None,
        max_entries: OptionalMaxEntries = None,
    ) -> str:
        """Route one contents operation to the view services."""
        _checked_operation(
            "read",
            operation,
            READ_OPERATIONS,
            {
                "path": path,
                "lines": lines,
                "context_lines": context_lines,
                "whole_file": whole_file,
                "depth": depth,
                "max_entries": max_entries,
            },
        )
        async with context_for(context, repo_root) as ctx:
            if operation == OPERATION_SLICE:
                intervals = _slice_intervals(lines, bool(whole_file), tool="read operation 'slice'")
                return handlers.read_slice(
                    ctx,
                    path or "",
                    intervals,
                    _or_default(context_lines, DEFAULT_CONTEXT_LINES),
                )
            # The remaining table entry is OPERATION_DIR; the note on the same
            # arm of `symbols` says what keeps this fall-through honest.
            return handlers.list_dir(
                ctx,
                path,
                _or_default(depth, DEFAULT_RENDER_DEPTH),
                _or_default(max_entries, DEFAULT_MAX_ENTRIES),
            )


def roots_file(raw: str) -> RootsFile:
    """Read one ``--roots-from`` file, or refuse it by name at startup.

    Foreign input, parsed here and nowhere else. ``utf-8-sig`` rather than
    ``utf-8`` because a BOM would otherwise survive into the first entry, where
    it turns an absolute path into a relative one and yields a root that
    silently never matches. ``OSError`` and ``UnicodeDecodeError`` are converted
    explicitly: ``UnicodeDecodeError`` is a ``ValueError``, which argparse
    swallows in favour of a generic "invalid value" message that loses the
    reason. The stat comes before the read so a write landing between the two
    leaves a recorded mtime older than the content, which costs one redundant
    re-read on the next call rather than a stale answer.
    """
    path = Path(raw)
    try:
        stat_result = path.stat()
        text = path.read_text(encoding="utf-8-sig")
    except OSError as exc:
        message = f"cannot read roots file {raw}: {exc}"
        raise argparse.ArgumentTypeError(message) from exc
    except UnicodeDecodeError as exc:
        message = f"roots file {raw} is not valid UTF-8: {exc}"
        raise argparse.ArgumentTypeError(message) from exc

    return RootsFile(
        path=path,
        roots=resolved_allowlist(_root_lines(text)),
        stat_key=(stat_result.st_mtime_ns, stat_result.st_size),
    )


def _looks_unsplit(element: str) -> bool:
    """Is this one argv element an option flag glued to its own value(s)?

    The server's parser declares no positional arguments, so an element whose
    first token lexes as an option and which lexes into more than one token
    cannot be anything but a shell string that was never word-split. Unbalanced
    quotes make the element unlexable, which is not evidence either way, and
    raising here would replace argparse's exit 2 with a traceback.
    """
    try:
        tokens = shlex.split(element)
    except ValueError:
        return False
    return len(tokens) > 1 and tokens[0].startswith("-")


def _report_argv(argv: Sequence[str]) -> None:
    """Describe the argv argparse just rejected, on stderr, for a captured log."""
    sys.stderr.write(f"agentless-mcp-server: received argv: {list(argv)!r}\n")
    for index, element in enumerate(argv):
        if not _looks_unsplit(element):
            continue
        sys.stderr.write(
            f"agentless-mcp-server: argv element {index} looks like an unsplit "
            "shell string; check quoting in the client registration. Pass each "
            'token as its own argv element (e.g. "--root", "/a", "--root", '
            '"/b"), or use --roots-from FILE.\n'
        )


def _loopback_only(host: str) -> bool:
    """Does every address ``host`` resolves to sit on the loopback interface?

    Resolution rather than a string comparison: ``localhost``, ``127.0.0.1``,
    ``::1`` and a hosts-file alias are all the same decision, and a name that
    resolves to a routable address is that decision's opposite however
    local it looks. A name that resolves to nothing is not loopback either --
    the caller reports it as a refusal rather than binding something else.
    """
    try:
        candidates = socket.getaddrinfo(host, None, proto=socket.IPPROTO_TCP)
    except socket.gaierror:
        return False
    if not candidates:
        return False
    return all(ipaddress.ip_address(info[4][0]).is_loopback for info in candidates)


def _check_transport(parser: argparse.ArgumentParser, args: argparse.Namespace) -> None:
    """Refuse a transport/binding combination at startup rather than at first use.

    Two refusals, both about a flag that would otherwise be silently ignored
    or silently obeyed:

    ``--host``/``--port`` under stdio have no meaning -- there is no socket --
    and an operator who passes them believes they got a listener. Saying so at
    startup costs one line; discovering it costs a debugging session against a
    port nothing is on.

    A non-loopback bind is refused outright because this server authenticates
    nobody. Its entire access-control story is the ``--root`` allowlist, which
    decides *which repositories* are readable and says nothing about *who* may
    read them. On a routable address that is not an allowlist at all: it is
    unauthenticated read access to the source of every enrolled repository.
    Anything wider than loopback therefore needs an authenticating proxy in
    front, which is a deployment decision this process cannot make for itself.
    """
    if args.transport == TRANSPORT_STDIO:
        passed = (("--host", args.host), ("--port", args.port))
        given = [flag for flag, value in passed if value is not None]
        if given:
            parser.error(
                f"{' and '.join(given)} apply to --transport {TRANSPORT_HTTP} and the "
                f"transport is {TRANSPORT_STDIO}, which has no socket to bind. Pass "
                f"--transport {TRANSPORT_HTTP} to serve over HTTP, or drop the flag."
            )
        return
    if args.allow_client_roots:
        parser.error(
            f"--allow-client-roots cannot be combined with --transport {TRANSPORT_HTTP}. "
            "That flag lets the connected client's advertised roots authorise "
            "repositories, which is safe under stdio because the client is the process "
            "that spawned this server. Over HTTP the client is whatever reaches the "
            "port, and loopback is not per-user isolated, so any local process could "
            "name its own root and read anything this server's user can read -- the "
            "--root allowlist would stop deciding what is servable. Drop "
            "--allow-client-roots and enrol the repositories with --root or "
            "--roots-from."
        )
    host = args.host if args.host is not None else DEFAULT_HTTP_HOST
    if not _loopback_only(host):
        parser.error(
            f"--host {host!r} is not a loopback address. This server authenticates no "
            "one: the --root allowlist decides which repositories are readable, not who "
            "may read them, so binding it where the network can reach it publishes every "
            f"enrolled repository. Bind {DEFAULT_HTTP_HOST} and put an authenticating "
            "proxy in front if you need it off-host."
        )


def http_binding(args: argparse.Namespace) -> tuple[str, int]:
    """The address the HTTP transport listens on, defaults filled in.

    One place resolves the ``None`` sentinels that let ``_check_transport``
    tell "the operator passed this" from "the operator said nothing", so the
    listener and the refusal can never disagree about what the default is.

    A hostname is resolved here to the literal that was checked, and the
    literal is what gets bound. Passing the name through would leave two
    independent lookups between the loopback check and the socket -- this
    one and the server stack's own at bind time -- and a name whose records
    change in between (a short TTL, a round-robin mixing loopback with a
    routable address) would pass the check and bind the other answer. What
    was verified has to be what is used.
    """
    host = args.host if args.host is not None else DEFAULT_HTTP_HOST
    port = args.port if args.port is not None else DEFAULT_HTTP_PORT
    return _loopback_literal(host), port


def _loopback_literal(host: str) -> str:
    """The checked loopback address for ``host`` as an IP literal.

    Falls back to the name only when it resolves to nothing at all, which
    ``_check_transport`` has already refused for every path that reaches
    here; returning it unchanged keeps this from inventing an address.
    """
    try:
        candidates = socket.getaddrinfo(host, None, proto=socket.IPPROTO_TCP)
    except socket.gaierror:
        return host
    for info in candidates:
        address = str(info[4][0])
        if ipaddress.ip_address(address).is_loopback:
            return address
    return host


def parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    """Parse the server's own command line."""
    parser = argparse.ArgumentParser(
        prog="agentless-mcp-server",
        description="Read-only MCP server over the agentless-mcp read surface.",
    )
    parser.add_argument(
        "--root",
        action="append",
        default=[],
        metavar="DIR",
        help="a repository this server may serve; repeatable",
    )
    parser.add_argument(
        "--roots-from",
        action="append",
        default=[],
        metavar="FILE",
        type=roots_file,
        help=(
            "a file of repository paths, one per line, added to --root; blank "
            "lines and #-comment lines are skipped and relative paths resolve "
            "against the working directory, as --root does; repeatable"
        ),
    )
    parser.add_argument(
        "--allow-client-roots",
        action="store_true",
        help=(
            "let the connected client's advertised MCP roots authorise "
            "repositories, not merely select among --root directories. This "
            "hands the client the operator's decision about what is servable"
        ),
    )
    parser.add_argument(
        "--no-auto-warm",
        action="store_true",
        help="do not warm cold grammars in the background at startup; grammars "
        f"then warm only through agentless-mcp warmup ({grammars.ENV_NO_AUTO_WARM} "
        "in the environment does the same)",
    )
    parser.add_argument(
        "--no-auto-index",
        action="store_true",
        help="do not refresh a stale tag cache in the background when a "
        "repository is first served; the cache then updates only through "
        f"agentless-mcp index ({cache.ENV_NO_AUTO_INDEX} in the environment "
        "does the same)",
    )
    parser.add_argument(
        "--no-auto-restart",
        action="store_true",
        help="do not restart the HTTP server when its installed package "
        "changes; the process then serves the code it loaded at startup "
        f"until restarted by hand ({selfrestart.ENV_NO_AUTO_RESTART} in the "
        "environment does the same)",
    )
    parser.add_argument(
        "--surface",
        choices=SURFACES,
        default=SURFACE_V2,
        help=(
            "which tool surface to publish: v2 (the default) is the five "
            "consolidated intent-shaped tools, v1 is the original eleven for "
            "un-migrated operators, both publishes the union for a transition "
            "window"
        ),
    )
    parser.add_argument(
        "--transport",
        choices=TRANSPORTS,
        default=TRANSPORT_STDIO,
        help=(
            "how the server talks to its client: stdio (the default) for a "
            "client that launches this process, http for one long-lived server "
            "several clients share over FastMCP's streamable-http transport"
        ),
    )
    parser.add_argument(
        "--host",
        default=None,
        metavar="ADDR",
        help=(
            f"address --transport {TRANSPORT_HTTP} binds; loopback only, "
            f"default {DEFAULT_HTTP_HOST}"
        ),
    )
    parser.add_argument(
        "--port",
        default=None,
        type=int,
        metavar="N",
        help=f"port --transport {TRANSPORT_HTTP} binds; default {DEFAULT_HTTP_PORT}",
    )
    try:
        args = parser.parse_args(argv)
        # Inside the try on purpose: parser.error leaves by the same SystemExit
        # door argparse itself uses, so a transport refusal gets the argv
        # diagnostic below rather than reading to the operator as a dead socket.
        _check_transport(parser, args)
    except SystemExit as exc:
        # Under an MCP client the exit-2 usage error is invisible and the whole
        # session reads as a closed connection, so the argv itself is the
        # diagnostic. --help and --version leave by the same door with code 0.
        if exc.code not in (0, None):
            _report_argv(list(sys.argv[1:] if argv is None else argv))
        raise
    return args


def serve(argv: Sequence[str] | None, services: ServerServices) -> int:
    """Start the server. Returns only when the transport closes."""
    args = parse_args(argv)
    handlers = ToolHandlers(
        resolved_allowlist(args.root),
        services,
        allow_client_roots=args.allow_client_roots,
        roots_files=args.roots_from,
        auto_index=not args.no_auto_index,
    )
    server = build_server(handlers, surface=args.surface)
    # Non-blocking on purpose: MCP clients auto-spawn stdio servers, so the
    # process must serve immediately; until the warm lands, answers carry the
    # labeled skips with the warm-in-progress reason. AGENTLESS_MCP_NO_DOWNLOAD
    # keeps absolute priority inside start_auto_warm.
    if not args.no_auto_warm:
        grammars.start_auto_warm()
    if args.transport == TRANSPORT_HTTP:
        # Only the long-running transport can drift from its install: stdio
        # processes are per-connection and load new code on reconnect.
        if not args.no_auto_restart:
            selfrestart.start_update_monitor(DISTRIBUTION_NAME)
        host, port = http_binding(args)
        try:
            server.run(transport=TRANSPORT_HTTP, host=host, port=port)
        except KeyboardInterrupt:
            # The monitor's SIGINT may surface as KeyboardInterrupt rather than
            # as a handled transport shutdown, and exactly one interrupt is its
            # own. Claiming it is what tells the two sources apart: a restart
            # being pending says only that the monitor fired at some point, so
            # keying on that alone absorbed an operator's Ctrl+C landing any
            # time afterwards and restarted a server they meant to stop.
            if not selfrestart.claim_monitor_interrupt():
                raise
        if selfrestart.restart_pending():
            return selfrestart.exec_or_exit()
    else:
        server.run()
    return 0
