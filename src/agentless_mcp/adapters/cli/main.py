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
import json
import sys
from collections.abc import Callable, Sequence
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

from agentless_mcp.adapters.cli.formatting import (
    EXIT_DOMAIN,
    EXIT_OK,
    EXIT_USAGE,
    emit,
    exit_code_for,
    fail,
    note,
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
from agentless_mcp.application.patch_service import CheckReport, PatchService, load_edits
from agentless_mcp.application.repo_context import RepoContext, resolve_repo
from agentless_mcp.application.symbol_service import (
    DEFAULT_EXPAND_LIMIT,
    DEFAULT_FIND_LIMIT,
    DEFAULT_REFS_LIMIT,
    SymbolService,
    kind_names,
)
from agentless_mcp.application.validate_service import (
    DEFAULT_JOBS,
    DEFAULT_REPEAT_BASELINE,
    DEFAULT_TIMEOUT_SECONDS,
    BaselineStatus,
    ValidateRequest,
    ValidateService,
    load_verdicts,
)
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import cache, grammars, projectconfig, vote
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.gitinfo import git_root
from agentless_mcp.core.locs import DEFAULT_CONTEXT_LINES
from agentless_mcp.core.patches import ApplyResult, Edit
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
from agentless_mcp.util.tokens import (
    COUNTER_CHARS4,
    COUNTER_TIKTOKEN,
    TOKEN_COUNTERS,
    TokenCounter,
)

AUTO_BUDGET = "auto"

# How many per-file index failures the summary prints before it stops listing
# them. The count in the summary line is always complete.
INDEX_FAILURE_LINES = 10


@dataclass(frozen=True)
class CliServices:
    """The application services one CLI process needs, wired by bootstrap.

    The extractor is here as well as inside the services because opening a
    repository's tag cache is the adapter's job: which repository a call is
    about, and whether it passed ``--no-cache``, are both facts the adapter
    already holds and the services deliberately do not.
    """

    maps: MapService
    views: ViewService
    symbols: SymbolService
    patches: PatchService
    validates: ValidateService
    counter: TokenCounter
    extractor: TreeSitterExtractor


def run(argv: Sequence[str] | None, services: CliServices) -> int:
    """Parse ``argv`` and execute one subcommand."""
    parser = build_parser()
    args = parser.parse_args(argv)

    handler: Callable[[argparse.Namespace, CliServices], int] = args.handler
    try:
        return handler(args, services)
    except AtlasError as error:
        return fail(str(error), exit_code_for(error))


def counter_parser() -> argparse.ArgumentParser:
    """Return the parser owning ``--token-counter``, declared once.

    Which token counter a process uses is a composition-root decision -- the
    counter is constructed before any subcommand runs -- but the flag that
    selects it is still command-line syntax and belongs here. Declaring it in
    one place and giving it to both readers is what keeps the two from
    drifting into accepting different spellings.
    """
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument(
        "--token-counter",
        choices=TOKEN_COUNTERS,
        default=None,
        help=f"how token budgets are estimated (default: {COUNTER_CHARS4}). "
        f"'{COUNTER_TIKTOKEN}' needs the 'tokens' extra and shifts every budget",
    )
    return parser


def build_parser() -> argparse.ArgumentParser:
    """Build the full subcommand tree."""
    parser = argparse.ArgumentParser(
        prog="agentless-mcp",
        description="Model-free tree-sitter repo map, localization and slice machinery.",
        parents=[counter_parser()],
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
    _add_patch(subparsers)
    _add_validate(subparsers)
    _add_vote(subparsers)
    _add_warmup(subparsers)
    _add_index(subparsers)
    _add_capabilities(subparsers)
    return parser


# ---------------------------------------------------------------------------
# Subcommand wiring
# ---------------------------------------------------------------------------


def _repo_flags(parser: argparse.ArgumentParser, *, cache_flag: bool = True) -> None:
    """Add the flags every repository-scoped subcommand shares."""
    parser.add_argument(
        "--repo",
        metavar="PATH",
        default=None,
        help="repository root (default: the git root enclosing the current directory)",
    )
    parser.add_argument("--json", action="store_true", help="emit JSON instead of text")
    if cache_flag:
        parser.add_argument(
            "--no-cache",
            action="store_true",
            help="parse on demand and ignore the tag cache for this call",
        )


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
    # No argparse defaults on the three keys `.agentless-mcp.json` can set:
    # the precedence rule is explicit argument > project config > built-in
    # default, and a default filled in by argparse is indistinguishable from
    # one the caller typed.
    parser.add_argument(
        "--budget",
        default=None,
        help=f"token budget for the map body, or '{AUTO_BUDGET}' to size it from the repository "
        f"(default: {AUTO_BUDGET})",
    )
    parser.add_argument(
        "--max-files", type=int, default=None, help=f"(default: {DEFAULT_MAX_FILES})"
    )
    parser.add_argument(
        "--granularity",
        choices=GRANULARITIES,
        default=None,
        help=f"(default: {GRANULARITY_FUNCTION})",
    )
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
        default=None,
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


def _add_patch(subparsers: Any) -> None:
    """Wire the write side: parse, check, apply and normalise SEARCH/REPLACE edits.

    CLI only, by design. These four subcommands are the whole write surface of
    the package and the MCP server does not expose any of them, so writing to
    a repository always costs an explicit Bash invocation rather than a tool
    call an analysed repository's own contents could provoke.
    """
    parser = subparsers.add_parser("patch", help="SEARCH/REPLACE patch machinery (write side)")
    commands = parser.add_subparsers(dest="patch_command", required=True)

    parse = commands.add_parser("parse", help="turn SEARCH/REPLACE text into edits.json")
    _patch_input_flag(parse)
    parse.set_defaults(handler=_cmd_patch_parse)

    check = commands.add_parser("check", help="apply in memory and report syntax deltas")
    _patch_input_flag(check)
    _repo_flags(check, cache_flag=False)
    check.set_defaults(handler=_cmd_patch_check)

    apply_parser = commands.add_parser("apply", help="apply in a worktree and emit the diff")
    _patch_input_flag(apply_parser)
    _repo_flags(apply_parser, cache_flag=False)
    apply_parser.add_argument(
        "--in-place",
        action="store_true",
        help="write to the checkout instead of a scratch worktree; requires a clean tree",
    )
    apply_parser.set_defaults(handler=_cmd_patch_apply)

    normalize = commands.add_parser("normalize", help="AST-equivalence key for the change")
    _patch_input_flag(normalize)
    _repo_flags(normalize, cache_flag=False)
    normalize.set_defaults(handler=_cmd_patch_normalize)


def _patch_input_flag(parser: argparse.ArgumentParser) -> None:
    """Add the patch-input flag every write subcommand shares."""
    parser.add_argument(
        "-f",
        "--file",
        metavar="FILE",
        default=None,
        help="read the patch from FILE (SEARCH/REPLACE text or edits.json); default: stdin",
    )


def _add_validate(subparsers: Any) -> None:
    """Wire ``validate``: baseline, then one bounded test run per candidate.

    ``--repro-cmd`` is a flag and only a flag. ``--test-cmd`` is a flag with
    exactly one fallback: a ``test_cmd`` in the repository's own
    ``.agentless-mcp.json``, used only when the invocation named none, only
    here in the CLI -- no MCP tool can reach it -- and always printed in the
    run header before it is executed. There is still no ``Makefile``
    sniffing, no ``package.json`` scripts lookup and no built-in default: a
    command from the repository being judged runs only when a human asked for
    a validation run in that repository and can see which command it chose.
    """
    parser = subparsers.add_parser("validate", help="run candidate patches against the tests")
    parser.add_argument(
        "--candidates",
        required=True,
        metavar="DIR",
        help="directory of candidate patches, one file each (edits.json or SEARCH/REPLACE)",
    )
    parser.add_argument(
        "--repo",
        metavar="PATH",
        default=None,
        help="repository root (default: the git root enclosing the current directory)",
    )
    parser.add_argument(
        "--test-cmd",
        default=None,
        metavar="CMD",
        help="the regression command; must pass on unpatched HEAD or the run is UNVERIFIED. "
        "Falls back to test_cmd in the repository's .agentless-mcp.json, if it has one",
    )
    parser.add_argument(
        "--repro-cmd",
        default=None,
        metavar="CMD",
        help="the reproduction command; must FAIL on unpatched HEAD to count",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=DEFAULT_TIMEOUT_SECONDS,
        help=f"per-command hard bound in seconds (default: {DEFAULT_TIMEOUT_SECONDS}); "
        "a command that hits it is a failure, never a pass",
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=DEFAULT_JOBS,
        help=f"candidates to run concurrently, each in its own worktree (default: {DEFAULT_JOBS})",
    )
    parser.add_argument(
        "--repeat-baseline",
        type=int,
        default=DEFAULT_REPEAT_BASELINE,
        metavar="N",
        help=f"run the baseline N times before any candidate (default: {DEFAULT_REPEAT_BASELINE}); "
        "runs that disagree make the whole validation UNVERIFIED",
    )
    parser.add_argument(
        "-o",
        "--output",
        metavar="FILE",
        default=None,
        help="write the verdicts document here instead of stdout",
    )
    parser.set_defaults(handler=_cmd_validate)


def _add_vote(subparsers: Any) -> None:
    parser = subparsers.add_parser("vote", help="rank validated candidates by equivalence cluster")
    parser.add_argument("--verdicts", required=True, metavar="FILE", help="a validate output file")
    parser.add_argument("--json", action="store_true", help="emit JSON instead of text")
    parser.set_defaults(handler=_cmd_vote)


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
    parser = subparsers.add_parser("index", help="build or refresh the tag cache")
    _repo_flags(parser, cache_flag=False)
    parser.add_argument(
        "--force",
        action="store_true",
        help="re-extract every file instead of reusing rows whose sha256 still matches",
    )
    parser.set_defaults(handler=_cmd_index)


def _add_capabilities(subparsers: Any) -> None:
    parser = subparsers.add_parser("capabilities", help="grammars, versions and caps in force")
    _repo_flags(parser)
    parser.set_defaults(handler=_cmd_capabilities)


# ---------------------------------------------------------------------------
# Handlers
# ---------------------------------------------------------------------------


def _cmd_map(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    budget, refusal = _map_budget(args.budget, ctx)
    if refusal:
        return fail(refusal, EXIT_USAGE)

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
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    view = services.views.tree(ctx, depth=args.depth, max_entries=args.max_entries)
    _emit(args, ctx, services, _Answer(view.text, view.as_dict()))
    return EXIT_OK


def _cmd_skeleton(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    views = services.views.skeleton(
        ctx,
        args.files,
        docstrings=projectconfig.resolve(args.docstrings, ctx.config.docstrings, False),
        numbered=args.numbers,
    )
    text = "\n".join(f"### {view.path}\n{view.text or view.error}" for view in views)
    _emit(args, ctx, services, _Answer(text, {"files": [v.as_dict() for v in views]}, "files"))
    return EXIT_OK


def _cmd_expand(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args, services)
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
    ctx = _context(args, services)
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
    ctx = _context(args, services)
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
    ctx = _context(args, services)
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
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    view = services.views.resolve_locations(ctx, args.file, args.loc, context=args.context)
    text = _render_locations(view)
    _emit(args, ctx, services, _Answer(text, view.as_dict()))
    return EXIT_OK


def _cmd_patch_parse(args: argparse.Namespace, services: CliServices) -> int:
    """Parse SEARCH/REPLACE text into the edits.json document on stdout."""
    text = _patch_text(args)
    if text is None:
        return EXIT_USAGE

    result = services.patches.parse(text)
    emit(json.dumps(result.as_dict(), indent=2))
    if result.errors:
        note(f"agentless-mcp: {len(result.errors)} blocks did not parse")
        for error in result.errors:
            note(f"  block {error.index} ({error.path or 'no path'}): {error.reason}")
        return EXIT_DOMAIN
    return EXIT_OK


def _cmd_patch_check(args: argparse.Namespace, services: CliServices) -> int:
    """Apply edits in memory and report each edited file's syntax delta."""
    prepared = _patch_call(args, services)
    if prepared is None:
        return EXIT_USAGE

    ctx, edits = prepared
    report = services.patches.check(edits, ctx)

    if args.json:
        emit(json.dumps(report.as_dict(), indent=2))
    else:
        emit("\n".join(_check_lines(report)))

    _patch_receipt(ctx, report.summary_line(), report.result)
    return EXIT_OK if report.ok else EXIT_DOMAIN


def _cmd_patch_apply(args: argparse.Namespace, services: CliServices) -> int:
    """Apply edits and emit the unified diff on stdout."""
    prepared = _patch_call(args, services)
    if prepared is None:
        return EXIT_USAGE

    ctx, edits = prepared
    report = services.patches.apply(edits, ctx, in_place=args.in_place)

    if args.json:
        emit(json.dumps(report.as_dict(), indent=2))
    elif report.diff:
        emit(report.diff)

    _patch_receipt(ctx, report.summary_line(), report.result)
    return EXIT_OK if report.ok else EXIT_DOMAIN


def _cmd_patch_normalize(args: argparse.Namespace, services: CliServices) -> int:
    """Emit the AST-equivalence key of the change the edits describe."""
    prepared = _patch_call(args, services)
    if prepared is None:
        return EXIT_USAGE

    ctx, edits = prepared
    report = services.patches.normalize(edits, ctx)

    if args.json:
        emit(json.dumps(report.as_dict(), indent=2))
    else:
        emit(report.key)

    _patch_receipt(ctx, report.summary_line(), report.result)
    return EXIT_OK if report.ok else EXIT_DOMAIN


def _cmd_validate(args: argparse.Namespace, services: CliServices) -> int:
    """Validate every candidate and emit the verdicts document.

    Exit ``0`` only when at least one candidate applied, kept an equivalence
    key and passed the regression suite -- the two decisive ladder tiers. An
    UNVERIFIED baseline and a run where nothing survived are both ``1``: the
    caller learned something, and it was not a fix.
    """
    ctx = _resolve(args, require_git=True)
    if ctx is None:
        return EXIT_USAGE

    bounds = _validate_bounds(args)
    if bounds:
        return fail(bounds, EXIT_USAGE)

    test_cmd = args.test_cmd if args.test_cmd is not None else ctx.config.test_cmd
    if test_cmd is None:
        return fail(
            "validate needs a test command: pass --test-cmd, or set test_cmd in the "
            f"repository's {projectconfig.CONFIG_FILENAME}",
            EXIT_USAGE,
        )
    if args.test_cmd is None:
        # Printed before the run, not after: this command came out of the
        # repository being judged, and the caller has to see which one it is.
        note(
            f"agentless-mcp: test command from {ctx.config.path}: {test_cmd}\n"
            "agentless-mcp: it comes from the repository under analysis; "
            "pass --test-cmd to override it."
        )

    report = services.validates.validate(
        ctx,
        ValidateRequest(
            candidates=Path(args.candidates),
            test_cmd=test_cmd,
            repro_cmd=args.repro_cmd,
            timeout=args.timeout,
            jobs=args.jobs,
            repeat_baseline=args.repeat_baseline,
        ),
    )

    document = report.jsonl()
    if args.output is None:
        emit(document)
    else:
        destination = Path(args.output)
        try:
            destination.write_text(document, encoding="utf-8")
        except OSError as error:
            return fail(f"cannot write {destination}: {error.strerror}", EXIT_USAGE)
        note(f"agentless-mcp: verdicts written to {destination}")

    note("\n".join([*envelope.receipt_lines(ctx), f"# {report.summary_line()}"]))
    for warning in report.warnings():
        note(f"agentless-mcp: {warning}")
    return EXIT_OK if report.any_passed else EXIT_DOMAIN


def _cmd_vote(args: argparse.Namespace, services: CliServices) -> int:
    """Rank a verdicts document by equivalence cluster."""
    _ = services
    source = Path(args.verdicts)
    try:
        text = source.read_text(encoding="utf-8")
    except OSError as error:
        return fail(f"cannot read {source}: {error.strerror}", EXIT_USAGE)

    loaded = load_verdicts(text)
    report = vote.rank(loaded.candidates, repro_valid=loaded.repro_valid)

    if args.json:
        emit(json.dumps(report.as_dict(), indent=2))
    else:
        emit(report.text())

    note(f"# agentless-mcp vote over {source}")
    note(f"# test-cmd: {loaded.test_cmd}")
    note(f"# {report.summary_line()}")
    if loaded.baseline is not BaselineStatus.OK:
        note(
            "agentless-mcp: this ranking comes from an UNVERIFIED run -- the test command "
            "did not pass on unpatched HEAD, so no candidate was evaluated."
        )
    return EXIT_OK


def _cmd_warmup(args: argparse.Namespace, services: CliServices) -> int:
    """Fetch and probe grammars, failing on anything the caller counts on.

    A language the caller named explicitly is one they need, so any
    degradation among those is a failure. In the default sweep the tier
    decides: a tier-1 grammar that will not load breaks the languages this
    package promises, while a tier-2 one costs that language alone and is
    reported as a warning -- which is what "degrade per language" has to mean
    at the exit code as well as inside the extractor.
    """
    _ = services
    requested = list(args.languages)
    report = grammars.warmup(requested or None, no_download=args.no_download)
    lines = [f"pack {report.pack_version}  cache {report.cache_dir}"]
    lines.extend(
        f"  {cap.name:<12} tier={cap.tier} abi={cap.abi_version or '-'} warmed={cap.warmed} "
        f"probe={cap.probe_ok} {cap.detail}".rstrip()
        for cap in report.languages
    )
    emit("\n".join(lines))

    fatal = report.degraded if requested else report.degraded_tier1
    if fatal:
        return fail(f"{len(fatal)} grammars degraded; see the rows above")
    if report.degraded:
        note(
            f"agentless-mcp: {len(report.degraded)} tier-2 grammars degraded; "
            "those languages are unavailable, the rest are unaffected"
        )
    return EXIT_OK


def _cmd_index(args: argparse.Namespace, services: CliServices) -> int:
    """Build or refresh the tag cache for one repository."""
    ctx = _resolve(args, require_git=False)
    if ctx is None:
        return EXIT_USAGE

    report = cache.build_index(
        ctx.root,
        services.extractor,
        tree_oid=ctx.tree_oid,
        head_sha=ctx.head_sha,
        force=args.force,
    )
    if args.json:
        emit(json.dumps(report.as_dict(), indent=2))
        return EXIT_OK

    lines = [report.summary_line()]
    lines.extend(
        f"  error: {failure.path}: {failure.reason}"
        for failure in report.failures[:INDEX_FAILURE_LINES]
    )
    if report.errors > INDEX_FAILURE_LINES:
        lines.append(f"  ... {report.errors - INDEX_FAILURE_LINES} more errors not listed")
    emit("\n".join(lines))
    return EXIT_OK


def _cmd_capabilities(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args, services, require_git=False)
    if ctx is None:
        return EXIT_USAGE

    status = _cache_status(ctx, services)
    capabilities = grammars.loaded_capabilities()
    payload: dict[str, Any] = {
        "pack_version": grammars.pack_version(),
        "grammar_cache": grammars.cache_dir(),
        "cache": status.as_dict(),
        "languages": [
            {
                "name": cap.name,
                "tier": cap.tier,
                "abi": cap.abi_version,
                "warmed": cap.warmed,
                "probe_ok": cap.probe_ok,
                "detail": cap.detail,
            }
            for cap in capabilities
        ],
        "extensions": dict(sorted(TreeSitterExtractor.SUPPORTED_EXTENSIONS.items())),
        "config": ctx.config.as_dict(),
        "caps": _caps(),
    }

    lines = [
        f"pack {payload['pack_version']}  grammar cache {payload['grammar_cache']}",
        f"tag cache: {status.receipt}",
        f"  path {status.path}  files {status.files}  tags {status.tags}",
        "languages:",
    ]
    lines.extend(
        f"  {cap.name:<12} tier={cap.tier} abi={cap.abi_version or '-'} "
        f"warmed={cap.warmed} probe={cap.probe_ok}"
        for cap in capabilities
    )
    lines.append("caps:")
    lines.extend(f"  {name} = {value}" for name, value in _caps().items())
    _emit(args, ctx, services, _Answer("\n".join(lines) + "\n", payload, "languages"))
    return EXIT_OK


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _patch_text(args: argparse.Namespace) -> str | None:
    """Read the patch text from ``--file`` or stdin, or report why not.

    The patch is the caller's own input rather than repository content, so it
    is read as given: containment applies to the paths *inside* it, which the
    service checks against the repository root before anything is opened.
    """
    if args.file is None or args.file == "-":
        return sys.stdin.read()

    source = Path(args.file)
    try:
        return source.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"cannot read {source}: {error.strerror}", EXIT_USAGE)
        return None


def _patch_call(
    args: argparse.Namespace, services: CliServices
) -> tuple[RepoContext, tuple[Edit, ...]] | None:
    """Resolve the repository and load the edits one write subcommand acts on."""
    _ = services
    ctx = _resolve(args, require_git=False)
    if ctx is None:
        return None

    text = _patch_text(args)
    if text is None:
        return None

    parsed = load_edits(text)
    for error in parsed.errors:
        note(f"agentless-mcp: block {error.index} ({error.path or 'no path'}): {error.reason}")
    return ctx, parsed.edits


def _check_lines(report: CheckReport) -> list[str]:
    """Render a check report as one line per edited file."""
    lines: list[str] = []
    for check in report.files:
        if check.verdict is None:
            lines.append(f"{check.path}: {check.error}")
            continue
        verdict = check.verdict
        status = "ok" if verdict.ok else "BROKEN"
        detail = f" -- {verdict.detail}" if verdict.detail else ""
        lines.append(
            f"{check.path}: {status} ({verdict.language}, errors "
            f"{verdict.old_errors} -> {verdict.new_errors}){detail}"
        )
    return lines or ["no files were edited"]


def _patch_receipt(ctx: RepoContext, summary: str, result: ApplyResult) -> None:
    """Put the receipt, the summary and every failed edit on stderr."""
    note("\n".join([*envelope.receipt_lines(ctx), f"# {summary}"]))
    for outcome in result.failures:
        edit = outcome.edit
        note(f"  {outcome.status.value}: {edit.path} block {edit.index}: {outcome.reason}")


def _cache_status(ctx: RepoContext, services: CliServices) -> cache.CacheStatus:
    """Describe the tag cache behind one call, counting its rows."""
    source = ctx.symbols if ctx.symbols is not None else cache.OnDemandSource(services.extractor)
    return source.status()


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


def _resolve(args: argparse.Namespace, *, require_git: bool) -> RepoContext | None:
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


def _context(
    args: argparse.Namespace,
    services: CliServices,
    *,
    require_git: bool = True,
) -> RepoContext | None:
    """Resolve the repository and open the symbol source this call reads from.

    Opening the cache is an adapter decision, not a service one: it needs the
    repository's realpath, the generation its git state was snapshotted at,
    and the caller's ``--no-cache``. Every failure to open one degrades to
    on-demand parsing with the reason in the receipt, so a read command never
    fails because of a cache.
    """
    ctx = _resolve(args, require_git=require_git)
    if ctx is None:
        return None

    source = cache.open_source(
        ctx.root,
        services.extractor,
        tree_oid=ctx.tree_oid,
        no_cache=args.no_cache,
    )
    return replace(ctx, symbols=source)


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


def _validate_bounds(args: argparse.Namespace) -> str:
    """Return why ``validate``'s numeric flags are unusable, or an empty string."""
    if args.timeout <= 0:
        return "--timeout takes a positive number of seconds"
    if args.jobs < 1:
        return "--jobs takes a positive integer"
    if args.repeat_baseline < 1:
        return "--repeat-baseline takes a positive integer"
    return ""


def _map_budget(raw: str | None, ctx: RepoContext) -> tuple[int | None, str]:
    """Resolve ``--budget``: a number, ``auto``, or the reason it is neither.

    ``None`` for the budget means auto-size, which is why this cannot go
    through :func:`_first`: "the caller said auto" and "nobody said anything"
    are the same value there and must not be the same decision here.
    """
    if raw is None:
        return (None if ctx.config.map_budget is None else ctx.config.map_budget), ""
    if raw == AUTO_BUDGET:
        return None, ""

    parsed = _positive_int(raw)
    if parsed is None:
        return None, f"--budget takes a positive integer or '{AUTO_BUDGET}'"
    return parsed, ""
