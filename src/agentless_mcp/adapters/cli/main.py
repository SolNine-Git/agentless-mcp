"""The ``agentless-mcp`` command line: one subcommand per read tool.

This is the front door any agent can reach over Bash, and it is the reason the
MCP server can stay thin: both adapters call the same application services and
neither owns any behaviour of its own. What lives here is argument parsing,
the repository default, and the mapping from typed errors to exit codes.

The repository root is the one piece of state the CLI is allowed to infer.
``--repo`` names it explicitly; without the flag it is the git root enclosing
the current directory, because in a CLI the cwd is an unambiguous statement of
intent. That inference is deliberately absent from the MCP server, where a
single process serves several repositories at once and a guessed root would be
a wrong-repository answer nobody asked for.
"""

import argparse
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from agentless_mcp.adapters.cli.formatting import (
    EXIT_OK,
    EXIT_USAGE,
    emit,
    exit_code_for,
    fail,
    warn_about,
)
from agentless_mcp.application import envelope, render
from agentless_mcp.application.map_service import (
    DEFAULT_MAX_FILES,
    GRANULARITIES,
    GRANULARITY_FUNCTION,
    MapRequest,
    MapService,
)
from agentless_mcp.application.repo_context import RepoContext, resolve_repo
from agentless_mcp.application.symbol_service import (
    DEFAULT_EXPAND_LIMIT,
    DEFAULT_FIND_LIMIT,
    DEFAULT_REFS_LIMIT,
    SymbolService,
    kind_names,
)
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import grammars
from agentless_mcp.core.gitinfo import git_root
from agentless_mcp.core.locs import DEFAULT_CONTEXT_LINES
from agentless_mcp.core.symbols import parse_stable_id
from agentless_mcp.core.treewalk import DEFAULT_MAX_ENTRIES, DEFAULT_RENDER_DEPTH
from agentless_mcp.util.errors import AtlasError
from agentless_mcp.util.fslimits import (
    DEFAULT_MAX_DEPTH,
    DEFAULT_MAX_FILE_BYTES,
)
from agentless_mcp.util.fslimits import (
    DEFAULT_MAX_FILES as WALK_MAX_FILES,
)
from agentless_mcp.util.tokens import TokenCounter

AUTO_BUDGET = "auto"


@dataclass(frozen=True)
class CliServices:
    """The application services one CLI process needs, wired by bootstrap."""

    maps: MapService
    views: ViewService
    symbols: SymbolService
    counter: TokenCounter


def run(argv: Sequence[str] | None, services: CliServices) -> int:
    """Parse ``argv`` and execute one subcommand."""
    parser = build_parser()
    args = parser.parse_args(argv)

    handler: Callable[[argparse.Namespace, CliServices], int] = args.handler
    try:
        return handler(args, services)
    except AtlasError as error:
        return fail(str(error), exit_code_for(error))


def build_parser() -> argparse.ArgumentParser:
    """Build the full subcommand tree."""
    parser = argparse.ArgumentParser(
        prog="agentless-mcp",
        description="Model-free tree-sitter repo map, localization and slice machinery.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    _add_map(subparsers)
    _add_tree(subparsers)
    _add_skeleton(subparsers)
    _add_expand(subparsers)
    _add_slice(subparsers)
    _add_find_symbol(subparsers)
    _add_refs(subparsers)
    _add_resolve_locs(subparsers)
    _add_warmup(subparsers)
    _add_index(subparsers)
    _add_capabilities(subparsers)
    return parser


# ---------------------------------------------------------------------------
# Subcommand wiring
# ---------------------------------------------------------------------------


def _repo_flags(parser: argparse.ArgumentParser) -> None:
    """Add the flags every repository-scoped subcommand shares."""
    parser.add_argument(
        "--repo",
        metavar="PATH",
        default=None,
        help="repository root (default: the git root enclosing the current directory)",
    )
    parser.add_argument("--json", action="store_true", help="emit JSON instead of text")


def _add_map(subparsers: Any) -> None:
    parser = subparsers.add_parser("map", help="ranked, token-budgeted repository map")
    _repo_flags(parser)
    parser.add_argument(
        "--focus",
        action="append",
        default=[],
        metavar="FILE_OR_SYMBOL",
        help="seed the ranking with a file or symbol; repeatable",
    )
    parser.add_argument(
        "--budget",
        default=AUTO_BUDGET,
        help=f"token budget for the map body, or '{AUTO_BUDGET}' to size it from the repository",
    )
    parser.add_argument("--max-files", type=int, default=DEFAULT_MAX_FILES)
    parser.add_argument("--granularity", choices=GRANULARITIES, default=GRANULARITY_FUNCTION)
    parser.set_defaults(handler=_cmd_map)


def _add_tree(subparsers: Any) -> None:
    parser = subparsers.add_parser("tree", help="gitignore-aware directory tree")
    _repo_flags(parser)
    parser.add_argument("--depth", type=int, default=DEFAULT_RENDER_DEPTH)
    parser.add_argument("--max-entries", type=int, default=DEFAULT_MAX_ENTRIES)
    parser.set_defaults(handler=_cmd_tree)


def _add_skeleton(subparsers: Any) -> None:
    parser = subparsers.add_parser("skeleton", help="signatures with bodies elided")
    _repo_flags(parser)
    parser.add_argument("files", nargs="+", metavar="FILE")
    parser.add_argument(
        "--docstrings",
        action="store_true",
        help="keep docstrings, truncated (off by default: tokens and injection surface)",
    )
    parser.add_argument("--numbers", action="store_true", help="prefix lines with N|")
    parser.set_defaults(handler=_cmd_skeleton)


def _add_expand(subparsers: Any) -> None:
    parser = subparsers.add_parser("expand", help="full bodies for named stable ids")
    _repo_flags(parser)
    parser.add_argument("ids", nargs="+", metavar="STABLE_ID")
    parser.add_argument("--limit", type=int, default=DEFAULT_EXPAND_LIMIT)
    parser.set_defaults(handler=_cmd_expand)


def _add_slice(subparsers: Any) -> None:
    parser = subparsers.add_parser("slice", help="numbered lines with scope headers")
    _repo_flags(parser)
    parser.add_argument("file", nargs="?", metavar="FILE")
    parser.add_argument(
        "--lines",
        action="append",
        default=[],
        metavar="A:B",
        help="1-based inclusive line range; repeatable and merged",
    )
    parser.add_argument("--symbol", metavar="STABLE_ID", help="slice this symbol instead")
    parser.add_argument("--context", type=int, default=DEFAULT_CONTEXT_LINES)
    parser.set_defaults(handler=_cmd_slice)


def _add_find_symbol(subparsers: Any) -> None:
    parser = subparsers.add_parser("find-symbol", help="substring or qualified symbol lookup")
    _repo_flags(parser)
    parser.add_argument("name", metavar="NAME")
    parser.add_argument("--kind", choices=kind_names(), default=None)
    parser.add_argument("--limit", type=int, default=DEFAULT_FIND_LIMIT)
    parser.set_defaults(handler=_cmd_find_symbol)


def _add_refs(subparsers: Any) -> None:
    parser = subparsers.add_parser("refs", help="fan-in: what references this symbol")
    _repo_flags(parser)
    parser.add_argument("target", metavar="NAME_OR_STABLE_ID")
    parser.add_argument("--limit", type=int, default=DEFAULT_REFS_LIMIT)
    parser.add_argument(
        "--shared-callers",
        action="store_true",
        help="also list symbols the same callers use (the DRY pass)",
    )
    parser.set_defaults(handler=_cmd_refs)


def _add_resolve_locs(subparsers: Any) -> None:
    parser = subparsers.add_parser(
        "resolve-locs", help="turn class:/function:/line: strings into intervals"
    )
    _repo_flags(parser)
    parser.add_argument("file", metavar="FILE")
    parser.add_argument("--loc", action="append", default=[], metavar="LOC", required=True)
    parser.add_argument("--context", type=int, default=DEFAULT_CONTEXT_LINES)
    parser.set_defaults(handler=_cmd_resolve_locs)


def _add_warmup(subparsers: Any) -> None:
    parser = subparsers.add_parser("warmup", help="fetch and probe grammars; fails loudly")
    parser.add_argument("languages", nargs="*", metavar="LANGUAGE")
    parser.add_argument(
        "--no-download",
        action="store_true",
        help="air-gap mode: a grammar that would need fetching is an error",
    )
    parser.set_defaults(handler=_cmd_warmup)


def _add_index(subparsers: Any) -> None:
    parser = subparsers.add_parser("index", help="build the tag cache (Phase 1.5)")
    _repo_flags(parser)
    parser.set_defaults(handler=_cmd_index)


def _add_capabilities(subparsers: Any) -> None:
    parser = subparsers.add_parser("capabilities", help="grammars, versions and caps in force")
    _repo_flags(parser)
    parser.set_defaults(handler=_cmd_capabilities)


# ---------------------------------------------------------------------------
# Handlers
# ---------------------------------------------------------------------------


def _cmd_map(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args)
    if ctx is None:
        return EXIT_USAGE

    budget = None if args.budget == AUTO_BUDGET else _positive_int(args.budget)
    if args.budget != AUTO_BUDGET and budget is None:
        return fail(f"--budget takes a positive integer or '{AUTO_BUDGET}'", EXIT_USAGE)

    result = services.maps.build(
        ctx,
        MapRequest(
            focus=tuple(args.focus),
            budget=budget,
            max_files=args.max_files,
            granularity=args.granularity,
        ),
    )
    _emit(
        args,
        ctx,
        services,
        _Answer(
            text=services.maps.render_text(result),
            payload=result.as_dict(),
            items_key="files",
            truncation=envelope.Truncation(
                shown=result.included, total=result.candidates, unit="symbols"
            ),
        ),
    )
    return EXIT_OK


def _cmd_tree(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args)
    if ctx is None:
        return EXIT_USAGE

    view = services.views.tree(ctx, depth=args.depth, max_entries=args.max_entries)
    _emit(args, ctx, services, _Answer(view.text, view.as_dict()))
    return EXIT_OK


def _cmd_skeleton(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args)
    if ctx is None:
        return EXIT_USAGE

    views = services.views.skeleton(
        ctx, args.files, docstrings=args.docstrings, numbered=args.numbers
    )
    text = "\n".join(f"### {view.path}\n{view.text or view.error}" for view in views)
    _emit(args, ctx, services, _Answer(text, {"files": [v.as_dict() for v in views]}, "files"))
    return EXIT_OK


def _cmd_expand(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args)
    if ctx is None:
        return EXIT_USAGE

    result = services.symbols.expand_symbols(ctx, list(args.ids), limit=args.limit)
    text = render.render_symbol_cards(result.cards)
    if result.unresolved:
        text += "\n" + "\n".join(
            f"unresolved: {entry} -- {reason}" for entry, reason in result.unresolved
        )
    _emit(args, ctx, services, _Answer(text, result.as_dict(), "symbols"))
    return EXIT_OK


def _cmd_slice(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args)
    if ctx is None:
        return EXIT_USAGE

    if args.symbol:
        parsed = parse_stable_id(args.symbol)
        located = services.views.resolve_locations(
            ctx, parsed.path, [f"function: {parsed.qualname}"], context=args.context
        )
        _emit(args, ctx, services, _Answer(located.text or "no such symbol\n", located.as_dict()))
        return EXIT_OK

    if not args.file:
        return fail("slice needs a FILE or --symbol STABLE_ID", EXIT_USAGE)

    intervals = _parse_ranges(args.lines)
    if intervals is None:
        return fail("--lines takes A:B with 1-based inclusive integers", EXIT_USAGE)

    view = services.views.read_slice(ctx, args.file, intervals=intervals, context=args.context)
    if view.error:
        return fail(view.error)
    _emit(args, ctx, services, _Answer(view.text, view.as_dict()))
    return EXIT_OK


def _cmd_find_symbol(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args)
    if ctx is None:
        return EXIT_USAGE

    result = services.symbols.find_symbol(ctx, args.name, kind=args.kind, limit=args.limit)
    _emit(
        args,
        ctx,
        services,
        _Answer(render.render_symbol_cards(result.cards), result.as_dict(), "matches"),
    )
    return EXIT_OK


def _cmd_refs(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args)
    if ctx is None:
        return EXIT_USAGE

    result = services.symbols.find_referencing_symbols(
        ctx, args.target, limit=args.limit, shared_callers=args.shared_callers
    )
    text = render.render_ref_groups(result.groups, args.target)
    if args.shared_callers:
        text += "\n" + render.render_shared_callers(result.shared, args.target)
    _emit(args, ctx, services, _Answer(text, result.as_dict(), "groups"))
    return EXIT_OK


def _cmd_resolve_locs(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args)
    if ctx is None:
        return EXIT_USAGE

    view = services.views.resolve_locations(ctx, args.file, args.loc, context=args.context)
    text = _render_locations(view)
    _emit(args, ctx, services, _Answer(text, view.as_dict()))
    return EXIT_OK


def _cmd_warmup(args: argparse.Namespace, services: CliServices) -> int:
    _ = services
    report = grammars.warmup(args.languages or None, no_download=args.no_download)
    lines = [f"pack {report.pack_version}  cache {report.cache_dir}"]
    lines.extend(
        f"  {cap.name:<12} abi={cap.abi_version or '-'} warmed={cap.warmed} "
        f"probe={cap.probe_ok} {cap.detail}".rstrip()
        for cap in report.languages
    )
    emit("\n".join(lines))
    if report.degraded:
        return fail(f"{len(report.degraded)} grammars degraded; see the rows above")
    return EXIT_OK


def _cmd_index(args: argparse.Namespace, services: CliServices) -> int:
    _ = args, services
    return fail(
        "index is not yet implemented: the tag cache lands in Phase 1.5. "
        "Every read command works index-free today.",
        EXIT_USAGE,
    )


def _cmd_capabilities(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args, require_git=False)
    if ctx is None:
        return EXIT_USAGE

    capabilities = grammars.loaded_capabilities()
    payload: dict[str, Any] = {
        "pack_version": grammars.pack_version(),
        "grammar_cache": grammars.cache_dir(),
        "cache": envelope.CACHE_GENERATION,
        "languages": [
            {
                "name": cap.name,
                "abi": cap.abi_version,
                "warmed": cap.warmed,
                "probe_ok": cap.probe_ok,
                "detail": cap.detail,
            }
            for cap in capabilities
        ],
        "caps": _caps(),
    }

    lines = [
        f"pack {payload['pack_version']}  grammar cache {payload['grammar_cache']}",
        f"tag cache: {envelope.CACHE_GENERATION} (index-free on-demand parsing)",
        "languages:",
    ]
    lines.extend(
        f"  {cap.name:<12} abi={cap.abi_version or '-'} warmed={cap.warmed} probe={cap.probe_ok}"
        for cap in capabilities
    )
    lines.append("caps:")
    lines.extend(f"  {name} = {value}" for name, value in _caps().items())
    _emit(args, ctx, services, _Answer("\n".join(lines) + "\n", payload, "languages"))
    return EXIT_OK


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _caps() -> dict[str, int]:
    """Return the bounds in force, so a caller can see why a view stopped."""
    return {
        "max_walk_depth": DEFAULT_MAX_DEPTH,
        "max_walk_files": WALK_MAX_FILES,
        "max_file_bytes": DEFAULT_MAX_FILE_BYTES,
        "max_output_tokens": envelope.DEFAULT_MAX_TOKENS,
        "max_map_files": DEFAULT_MAX_FILES,
        "max_expand_symbols": DEFAULT_EXPAND_LIMIT,
    }


def _context(args: argparse.Namespace, *, require_git: bool = True) -> RepoContext | None:
    """Resolve the repository this invocation is about, or report why not.

    ``allowlist=None`` throughout: in the CLI the root comes from the caller's
    own cwd or their own ``--repo``, which is exactly as trusted as the process
    itself. The allowlist exists for the server, where it is not.
    """
    if args.repo is not None:
        return resolve_repo(args.repo, None)

    cwd = Path.cwd()
    root = git_root(cwd)
    if root is None:
        if require_git:
            fail(
                f"{cwd} is not inside a git repository, so there is no root to default to; "
                "pass --repo PATH",
                EXIT_USAGE,
            )
            return None
        return resolve_repo(cwd, None)

    ctx = resolve_repo(root, None)
    warn_about(ctx)
    return ctx


@dataclass(frozen=True)
class _Answer:
    """One subcommand's result in both renderings, plus the trimmable list.

    Both forms are built before either is chosen so that `--json` cannot
    diverge from the text output by accident: the same values feed both.
    """

    text: str
    payload: dict[str, Any]
    items_key: str | None = None
    truncation: envelope.Truncation | None = None


def _emit(
    args: argparse.Namespace,
    ctx: RepoContext,
    services: CliServices,
    answer: _Answer,
) -> None:
    """Emit one answer in whichever form the caller asked for."""
    if args.json:
        emit(
            envelope.wrap_json(
                ctx, answer.payload, counter=services.counter, items_key=answer.items_key
            )
        )
    else:
        emit(
            envelope.wrap(ctx, answer.text, counter=services.counter, truncation=answer.truncation)
        )


def _render_locations(view: Any) -> str:
    """Render a location resolution as ids, intervals and the reasons for misses."""
    resolution = view.resolution
    lines = [f"file: {view.path}"]
    lines.extend(f"matched: {stable}" for stable in resolution.stable_ids)
    lines.append(
        "intervals: "
        + (", ".join(f"{start}-{end}" for start, end in resolution.intervals) or "none")
    )
    lines.extend(
        f"unrecognized: {entry.loc} -- {entry.reason}" for entry in resolution.unrecognized
    )
    if view.text:
        lines.append("")
        lines.append(view.text.rstrip("\n"))
    return "\n".join(lines) + "\n"


def _parse_ranges(values: Sequence[str]) -> list[tuple[int, int]] | None:
    """Parse ``A:B`` range strings, returning None on the first malformed one."""
    parsed: list[tuple[int, int]] = []
    for value in values:
        head, separator, tail = value.partition(":")
        if not separator:
            head, tail = value, value
        try:
            start, end = int(head), int(tail)
        except ValueError:
            return None
        if start < 1 or end < start:
            return None
        parsed.append((start, end))
    return parsed


def _positive_int(value: str) -> int | None:
    """Parse a positive integer, or None when the text is not one."""
    try:
        parsed = int(value)
    except ValueError:
        return None
    return parsed if parsed > 0 else None
