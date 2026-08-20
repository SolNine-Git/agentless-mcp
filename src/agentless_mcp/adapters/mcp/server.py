"""The stdio MCP server: the read tools, over the same application services.

This adapter owns two things the CLI does not, and nothing else.

**The allowlist.** One server process serves a workspace of repositories, so
there is no cwd to infer a root from and inferring one would be a
wrong-repository answer. Every tool therefore takes ``repo_root`` first and it
is checked, exactly, against the roots the server was started with. Those come
from repeatable ``--root DIR`` flags and from nowhere else: the client's own
MCP ``roots`` capability -- verified present in the installed FastMCP as
``Context.list_roots()`` -- is read, but an advertised root can only *select*
among the configured ones, never add one. A server started with no ``--root``
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
import logging
from collections.abc import AsyncIterator, Callable, Sequence
from contextlib import asynccontextmanager
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Annotated, Literal
from urllib.parse import unquote, urlparse

from fastmcp import Context, FastMCP
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
)
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import cache, projectconfig
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.locs import DEFAULT_CONTEXT_LINES
from agentless_mcp.core.mermaid import DEFAULT_MAX_NODES
from agentless_mcp.core.symbols import SymbolKind, stable_id
from agentless_mcp.core.treewalk import DEFAULT_MAX_ENTRIES, DEFAULT_RENDER_DEPTH
from agentless_mcp.prompts import MESSAGES, PARAMETER_DESCRIPTIONS, TOOL_DESCRIPTIONS
from agentless_mcp.util import fslimits
from agentless_mcp.util.errors import AtlasError, SecurityRefusal
from agentless_mcp.util.tokens import TokenCounter

logger = logging.getLogger(__name__)

SERVER_NAME = "agentless-mcp"

# A line range arrives as a two-element [start, end] list.
_RANGE_PAIR_LENGTH = 2

# How long a client gets to answer `roots/list`. It is a capability query
# answered from memory, so seconds is already generous; the number that
# matters is that there is one, because every tool call waits behind it.
_LIST_ROOTS_TIMEOUT_SECONDS = 2.0

OPERATION_PATH = "path"
OPERATION_CYCLES = "cycles"
OPERATION_COMMUNITIES = "communities"
OPERATION_DIAGRAM = "diagram"

# The one parameter every tool shares. Its description is prompt data like
# the tool descriptions; pydantic carries it into the published schema, which
# is the only documentation an arbitrary client is guaranteed to read.
RepoRoot = Annotated[str | None, Field(description=PARAMETER_DESCRIPTIONS["repo_root"])]
MapFocus = Annotated[list[str] | None, Field(description=PARAMETER_DESCRIPTIONS["map_focus"])]
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
DiagramFocus = Annotated[str, Field(description=PARAMETER_DESCRIPTIONS["diagram_focus"])]
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

ExpandLimit = Annotated[
    int,
    Field(ge=1, le=MAX_LIMIT, description=PARAMETER_DESCRIPTIONS["expand_limit"]),
]
FindLimit = Annotated[
    int,
    Field(ge=1, le=MAX_LIMIT, description=PARAMETER_DESCRIPTIONS["find_limit"]),
]
ReferenceLimit = Annotated[
    int,
    Field(ge=1, le=MAX_LIMIT, description=PARAMETER_DESCRIPTIONS["reference_limit"]),
]
ExplainLimit = Annotated[
    int,
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
        le=fslimits.DEFAULT_MAX_FILES,
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
    max_nodes: int = DEFAULT_MAX_NODES
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
    ) -> None:
        self._roots = tuple(roots)
        self._services = services
        self._allow_client_roots = allow_client_roots

    @property
    def roots(self) -> tuple[Path, ...]:
        """The roots this server was configured with."""
        return self._roots

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
        allowed = list(self._roots)
        if self._allow_client_roots:
            allowed = list(dict.fromkeys([*allowed, *client_roots]))
        if not allowed:
            message = MESSAGES.server_no_roots
            raise SecurityRefusal(message)

        if repo_root is None or not repo_root.strip():
            selected = _sole_selection(allowed, client_roots)
            if selected is not None:
                return self._with_source(resolve_repo(selected, allowed), no_cache=no_cache)
            listing = ", ".join(str(path) for path in allowed)
            message = MESSAGES.server_root_required.format(roots=listing)
            raise SecurityRefusal(message)

        return self._with_source(resolve_repo(repo_root, allowed), no_cache=no_cache)

    def _with_source(self, ctx: RepoContext, *, no_cache: bool) -> RepoContext:
        """Open this call's symbol source: the tag cache, or on-demand parsing."""
        source = cache.open_source(
            ctx.root,
            self._services.extractor,
            tree_oid=ctx.tree_oid,
            no_cache=no_cache,
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
        return self._wrap(ctx, render.render_symbol_cards(result.cards))

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
        eleven tools picks better than one picking between fourteen. The
        operation is validated here rather than by an enum on the wire so that
        a wrong value is answered with the list of right ones.
        """
        handler = _OPERATIONS.get(request.operation)
        if handler is None:
            listed = ", ".join(sorted(_OPERATIONS))
            message = MESSAGES.unknown_operation.format(
                operation=request.operation, operations=listed
            )
            raise AtlasError(message)
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
            configured_roots=self._roots,
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
        raise AtlasError(message)
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
    the tool that asked on its behalf. Every way the call can fail --
    unimplemented, timed out, malformed payload, dead transport -- means the
    same thing here, "no advertised roots", and leaves the static roots
    standing.
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

    path = Path(unquote(parsed.path))
    if not path.is_absolute():
        message = "path is not absolute"
        raise ValueError(message)

    resolved = path.resolve()
    if not resolved.is_dir():
        message = "not an existing directory"
        raise ValueError(message)
    return resolved


def _intervals(ranges: Sequence[Sequence[int]]) -> list[tuple[int, int]]:
    """Parse the wire's line ranges into intervals, or refuse the whole call.

    Refused rather than filtered: ``read_slice`` renders the whole file when
    no interval survives, so dropping a malformed range answers a bounded
    request with the substitute content this tool promises never to return.
    The schema rejects a range that is not two 1-based line numbers before it
    reaches here; the shape it cannot express is an end before its start.
    """
    intervals: list[tuple[int, int]] = []
    for pair in ranges:
        if len(pair) != _RANGE_PAIR_LENGTH or pair[1] < pair[0]:
            listed = ", ".join(str(number) for number in pair)
            message = (
                f"read_slice range [{listed}] is not a line range: each one is "
                "[start, end], 1-based and inclusive, with end at or after start."
            )
            raise AtlasError(message)
        intervals.append((pair[0], pair[1]))
    return intervals


def _slice_intervals(
    ranges: Sequence[Sequence[int]] | None,
    whole_file: bool,
) -> list[tuple[int, int]]:
    """Parse an explicit bounded slice or an explicit whole-file request."""
    if ranges is not None and whole_file:
        message = "read_slice accepts lines or whole_file=true, not both"
        raise AtlasError(message)
    if ranges is None and not whole_file:
        message = "read_slice requires non-empty lines or explicit whole_file=true"
        raise AtlasError(message)
    return [] if ranges is None else _intervals(ranges)


def build_server(handlers: ToolHandlers) -> FastMCP[None]:
    """Register every read tool on a FastMCP server and return it.

    Each tool's wire description is passed explicitly from
    ``agentless_mcp.prompts``: the text a model reads is prompt data, revised
    on its own terms, and FastMCP would otherwise publish whatever the
    docstring happened to say. The docstrings below are code documentation.
    """
    mcp: FastMCP[None] = FastMCP(SERVER_NAME)

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
                    focus=tuple(focus or ()),
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
        limit: ExpandLimit = DEFAULT_EXPAND_LIMIT,
        no_cache: NoCache = False,
    ) -> str:
        """Return the full body of each named symbol, line-numbered."""
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            return handlers.expand_symbols(ctx, stable_ids, limit)

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
        limit: FindLimit = DEFAULT_FIND_LIMIT,
        no_cache: NoCache = False,
    ) -> str:
        """Find symbols by substring or qualified name."""
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            return handlers.find_symbol(ctx, name, kind, limit)

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["find_referencing_symbols"],
        annotations=read_only("Find referencing symbols"),
    )
    async def find_referencing_symbols(
        context: Context,
        target: ReferenceTarget,
        repo_root: RepoRoot = None,
        limit: ReferenceLimit = DEFAULT_REFS_LIMIT,
        shared_callers: SharedCallers = False,
    ) -> str:
        """Find the symbols that reference a target, grouped by file."""
        async with context_for(context, repo_root) as ctx:
            return handlers.find_referencing_symbols(
                ctx, target, limit, shared_callers=shared_callers
            )

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["explain_symbol"],
        annotations=read_only("Explain symbol"),
    )
    async def explain_symbol(
        context: Context,
        target: ExplainTarget,
        repo_root: RepoRoot = None,
        limit: ExplainLimit = DEFAULT_EXPLAIN_LIMIT,
        no_cache: NoCache = False,
    ) -> str:
        """Render one symbol's definition site, tiered fan-out, fan-in and imports."""
        async with context_for(context, repo_root, no_cache=no_cache) as ctx:
            return handlers.explain_symbol(ctx, target, limit)

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
        focus: DiagramFocus = "",
        max_nodes: MaxNodes = DEFAULT_MAX_NODES,
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
                    focus=focus,
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

    @mcp.tool(
        description=TOOL_DESCRIPTIONS["capabilities"],
        annotations=read_only("Capabilities"),
    )
    async def capabilities(context: Context, repo_root: RepoRoot = None) -> str:
        """Report loaded grammars, cache state and the bounds in force."""
        roots = await effective_client_roots(context)
        ctx = handlers.resolve(repo_root, roots)
        try:
            return handlers.capabilities(ctx, roots)
        finally:
            ctx.close()

    return mcp


def parse_args(argv: Sequence[str] | None) -> argparse.Namespace:
    """Parse the server's own command line."""
    parser = argparse.ArgumentParser(
        prog="agentless-mcp-server",
        description="Read-only stdio MCP server over the agentless-mcp read surface.",
    )
    parser.add_argument(
        "--root",
        action="append",
        default=[],
        metavar="DIR",
        help="a repository this server may serve; repeatable",
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
    return parser.parse_args(argv)


def serve(argv: Sequence[str] | None, services: ServerServices) -> int:
    """Start the stdio server. Returns only when the transport closes."""
    args = parse_args(argv)
    handlers = ToolHandlers(
        resolved_allowlist(args.root),
        services,
        allow_client_roots=args.allow_client_roots,
    )
    build_server(handlers).run()
    return 0
