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
import os
import re
import sqlite3
import sys
import tempfile
from collections.abc import Callable, Sequence
from contextlib import ExitStack
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
from agentless_mcp.application.capability_service import (
    build_capability_report,
    render_capability_report,
)
from agentless_mcp.application.graph_service import (
    DEFAULT_COMMUNITY_LIMIT,
    DEFAULT_CYCLE_LIMIT,
    DEFAULT_EXPLAIN_LIMIT,
    DEFAULT_HEALTH_LIMIT,
    DEFAULT_MEMBER_LIMIT,
    DiagramRequest,
    GraphService,
    PathOptions,
)
from agentless_mcp.application.lint_service import LintService, load_candidates, load_diff
from agentless_mcp.application.map_service import (
    DEFAULT_MAX_FILES,
    GRANULARITIES,
    GRANULARITY_BODY,
    GRANULARITY_FUNCTION,
    MapRequest,
    MapService,
    build_body_map,
    render_body_map,
)
from agentless_mcp.application.patch_service import CheckReport, PatchService, load_edits
from agentless_mcp.application.repo_context import RepoContext, resolve_repo
from agentless_mcp.application.symbol_service import (
    DEFAULT_EXPAND_LIMIT,
    DEFAULT_FIND_LIMIT,
    DEFAULT_REFS_LIMIT,
    SymbolService,
    kind_names,
    render_expansion,
    render_find,
    render_refs,
    unresolved_lines,
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
from agentless_mcp.application.view_service import LocationView, ViewService
from agentless_mcp.core import (
    cache,
    communities,
    grammars,
    guide,
    htmlgraph,
    projectconfig,
    resolve,
    sandbox,
    vote,
)
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.gitinfo import git_root
from agentless_mcp.core.htmlgraph import HtmlExport
from agentless_mcp.core.locs import DEFAULT_CONTEXT_LINES
from agentless_mcp.core.mermaid import DEFAULT_DIAGRAM_EDGES, DEFAULT_DIAGRAM_NODES
from agentless_mcp.core.patches import ApplyResult, Edit
from agentless_mcp.core.symbols import StableId, parse_stable_id
from agentless_mcp.core.treewalk import DEFAULT_MAX_ENTRIES, DEFAULT_RENDER_DEPTH
from agentless_mcp.util.errors import AgentlessError, OperationFailed
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

# Environment names accepted by ``--pass-env``. Shell assignment syntax and
# unbounded lists are refused at the CLI boundary; the validated names are the
# only foreign values that reach the subprocess environment builder.
ENVIRONMENT_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
MAX_PASSTHROUGH_ENV = 32

HTML_CACHE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*\.html\Z")
MAX_HTML_CACHE_NAME = 128


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
    graphs: GraphService
    patches: PatchService
    validates: ValidateService
    lints: LintService
    counter: TokenCounter
    extractor: TreeSitterExtractor
    resources: ExitStack | None = None


def run(argv: Sequence[str] | None, services: CliServices) -> int:
    """Parse ``argv`` and execute one subcommand."""
    parser = build_parser()
    args = parser.parse_args(argv)

    handler: Callable[[argparse.Namespace, CliServices], int] = args.handler
    # The warmup command owns the cache while it runs, so it never races a
    # background warm of the same directory; every other subcommand starts
    # one and serves immediately with today's labeled skips until it lands.
    warm = None
    if not args.no_auto_warm and handler is not _cmd_warmup:
        warm = grammars.start_auto_warm()
    try:
        with ExitStack() as resources:
            invocation = replace(services, resources=resources)
            try:
                return handler(args, invocation)
            except AgentlessError as error:
                return fail(str(error), exit_code_for(error))
    finally:
        # One-shot process: exiting mid-extraction would kill the daemon
        # thread inside a cache write. Bounded by the warm's own deadline.
        if warm is not None:
            grammars.wait_for_auto_warm()


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
    parser.add_argument(
        "--no-auto-warm",
        action="store_true",
        help="do not warm cold grammars in the background at startup; grammars "
        f"then warm only through an explicit warmup ({grammars.ENV_NO_AUTO_WARM} "
        "in the environment does the same)",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    _add_map(subparsers)
    _add_tree(subparsers)
    _add_skeleton(subparsers)
    _add_expand(subparsers)
    _add_slice(subparsers)
    _add_find_symbol(subparsers)
    _add_refs(subparsers)
    _add_explain(subparsers)
    _add_path(subparsers)
    _add_cycles(subparsers)
    _add_communities(subparsers)
    _add_diagram(subparsers)
    _add_health(subparsers)
    _add_html(subparsers)
    _add_resolve_locs(subparsers)
    _add_patch(subparsers)
    _add_lint(subparsers)
    _add_validate(subparsers)
    _add_vote(subparsers)
    _add_warmup(subparsers)
    _add_index(subparsers)
    _add_capabilities(subparsers)
    _add_guide(subparsers)
    return parser


# ---------------------------------------------------------------------------
# Subcommand wiring
# ---------------------------------------------------------------------------


def _repo_flags(
    parser: argparse.ArgumentParser,
    *,
    cache_flag: bool = True,
    json_flag: bool = True,
) -> None:
    """Add the flags every repository-scoped subcommand shares."""
    parser.add_argument(
        "--repo",
        metavar="PATH",
        default=None,
        help="repository root (default: the git root enclosing the current directory)",
    )
    if json_flag:
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
        "--max-files",
        type=int,
        default=None,
        help="ranked files admitted to the map before symbol packing "
        f"(default: {DEFAULT_MAX_FILES})",
    )
    parser.add_argument(
        "--granularity",
        choices=GRANULARITIES,
        default=None,
        help=f"map detail: '{GRANULARITY_FUNCTION}' lists symbols within ranked files, "
        f"'file' reports ranked files only, '{GRANULARITY_BODY}' returns the file rows "
        f"plus the top symbols' whole bodies (default: {GRANULARITY_FUNCTION})",
    )
    parser.set_defaults(handler=_cmd_map)


def _add_tree(subparsers: Any) -> None:
    parser = subparsers.add_parser("tree", help="gitignore-aware directory tree")
    _repo_flags(parser)
    parser.add_argument(
        "--depth",
        type=int,
        default=DEFAULT_RENDER_DEPTH,
        help=f"directory levels rendered below the tree root (default: {DEFAULT_RENDER_DEPTH})",
    )
    parser.add_argument(
        "--max-entries",
        type=int,
        default=DEFAULT_MAX_ENTRIES,
        help="files and directories rendered before the tree reports truncation "
        f"(default: {DEFAULT_MAX_ENTRIES})",
    )
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
    # FILE plus --lines and --symbol are alternatives, and --symbol used to win
    # silently: `slice a.py --lines 1:3 --symbol py:b.py::f` sliced b.py whole
    # and never said the other two were dropped. Declared the way `lint`
    # declares the same shape, so argparse refuses the combination by name.
    source = parser.add_mutually_exclusive_group()
    source.add_argument("file", nargs="?", metavar="FILE")
    source.add_argument("--symbol", metavar="STABLE_ID", help="slice this symbol instead")
    parser.add_argument(
        "--lines",
        action="append",
        default=[],
        metavar="A:B",
        help="1-based inclusive line range; repeatable and merged; needs FILE",
    )
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
        help="instead rank symbols the same callers use (the DRY pass)",
    )
    parser.set_defaults(handler=_cmd_refs)


def _add_explain(subparsers: Any) -> None:
    parser = subparsers.add_parser("explain", help="one symbol card: definition, fan-out, fan-in")
    _repo_flags(parser)
    parser.add_argument("target", metavar="NAME_OR_STABLE_ID")
    parser.add_argument(
        "--limit",
        type=int,
        default=DEFAULT_EXPLAIN_LIMIT,
        help=f"rows per section, per tier (default: {DEFAULT_EXPLAIN_LIMIT})",
    )
    parser.set_defaults(handler=_cmd_explain)


def _add_path(subparsers: Any) -> None:
    parser = subparsers.add_parser("path", help="shortest resolved path between two symbols")
    _repo_flags(parser)
    parser.add_argument("source", metavar="FROM")
    parser.add_argument("target", metavar="TO")
    parser.add_argument(
        "--include-unique",
        action="store_true",
        help="also walk repository-wide unique-name edges; off by default because uniqueness "
        "is retrieval evidence, not binding evidence",
    )
    parser.add_argument(
        "--include-ambiguous",
        action="store_true",
        help="also walk name-only-ambiguous edges; off by default because a path built "
        "on a guessed binding reads like a finding and is not one",
    )
    parser.add_argument(
        "--max-visited",
        type=int,
        default=resolve.DEFAULT_MAX_VISITED,
        help=f"node bound on the search (default: {resolve.DEFAULT_MAX_VISITED}); "
        "hitting it is reported, never silently answered as 'no path'",
    )
    parser.set_defaults(handler=_cmd_path)


def _add_cycles(subparsers: Any) -> None:
    parser = subparsers.add_parser("cycles", help="module-level import cycles")
    _repo_flags(parser)
    parser.add_argument(
        "--limit",
        type=int,
        default=DEFAULT_CYCLE_LIMIT,
        help=f"cycles listed (default: {DEFAULT_CYCLE_LIMIT}); the count is always complete",
    )
    parser.set_defaults(handler=_cmd_cycles)


def _add_communities(subparsers: Any) -> None:
    parser = subparsers.add_parser("communities", help="which files belong together")
    _repo_flags(parser)
    parser.add_argument(
        "--resolution",
        type=float,
        default=None,
        help=f"modularity resolution (default: {communities.DEFAULT_RESOLUTION:g}); "
        "lower groups more coarsely, higher splits into more, smaller groups",
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=DEFAULT_COMMUNITY_LIMIT,
        help=f"communities listed (default: {DEFAULT_COMMUNITY_LIMIT}); "
        "the count is always complete",
    )
    parser.add_argument(
        "--members",
        type=int,
        default=DEFAULT_MEMBER_LIMIT,
        help=f"member files listed per community (default: {DEFAULT_MEMBER_LIMIT})",
    )
    parser.set_defaults(handler=_cmd_communities)


def _add_diagram(subparsers: Any) -> None:
    """Wire ``diagram``: mermaid on stdout, everything about the run on stderr.

    The answer here is a *document fragment*, so it follows the same split the
    write subcommands use rather than the read ones: the diagram goes to
    stdout with no receipt in front of it, ready to be pasted into a file
    behind a fence of the caller's choosing, and the receipt, the caveat and
    the elision count go to stderr.
    """
    parser = subparsers.add_parser("diagram", help="mermaid flowchart of the module graph")
    _repo_flags(parser)
    parser.add_argument(
        "--focus",
        metavar="FILE_OR_SYMBOL",
        default=None,
        help="draw only this module's neighbourhood, resolved like map --focus",
    )
    parser.add_argument(
        "--max-nodes",
        type=int,
        default=DEFAULT_DIAGRAM_NODES,
        help=f"modules drawn (default: {DEFAULT_DIAGRAM_NODES}); the rest are announced "
        "on an explicit elision node",
    )
    parser.add_argument(
        "--max-edges",
        type=int,
        default=DEFAULT_DIAGRAM_EDGES,
        help=f"arrows drawn (default: {DEFAULT_DIAGRAM_EDGES}); reference edges past this "
        "bound are announced in a comment rather than drawn",
    )
    parser.add_argument(
        "--communities",
        action="store_true",
        help="group the modules into community subgraphs",
    )
    parser.add_argument(
        "--resolution",
        type=float,
        default=None,
        help="modularity resolution used by --communities",
    )
    parser.add_argument(
        "--check",
        metavar="FILE",
        default=None,
        help="regenerate and compare against FILE instead of printing; exit 0 when identical, "
        "1 when it has drifted. A leading ```mermaid fence is stripped before comparing, so a "
        "committed .md diagram can be checked as it stands",
    )
    parser.set_defaults(handler=_cmd_diagram)


def _add_health(subparsers: Any) -> None:
    parser = subparsers.add_parser("health", help="orphan candidates, unused exports and hubs")
    _repo_flags(parser)
    parser.add_argument(
        "--limit",
        type=int,
        default=DEFAULT_HEALTH_LIMIT,
        help=f"rows listed per section (default: {DEFAULT_HEALTH_LIMIT}); "
        "every section's count is always complete",
    )
    parser.set_defaults(handler=_cmd_health)


def _add_html(subparsers: Any) -> None:
    """Wire the optional human graph export; it is deliberately not an MCP tool."""
    parser = subparsers.add_parser("html", help="interactive HTML export of the module graph")
    _repo_flags(parser, json_flag=False)
    parser.add_argument(
        "--max-nodes",
        type=int,
        default=htmlgraph.DEFAULT_HTML_NODES,
        help=f"modules included (default: {htmlgraph.DEFAULT_HTML_NODES}, "
        f"maximum: {htmlgraph.MAX_HTML_NODES})",
    )
    parser.add_argument(
        "--max-edges",
        type=int,
        default=htmlgraph.DEFAULT_HTML_EDGES,
        help=f"edges included (default: {htmlgraph.DEFAULT_HTML_EDGES}, "
        f"maximum: {htmlgraph.MAX_HTML_EDGES})",
    )
    parser.add_argument(
        "--resolution",
        type=float,
        default=None,
        help="modularity resolution used for community colours",
    )
    parser.add_argument(
        "--cache-file",
        metavar="NAME.html",
        default=None,
        help="write under this repository's XDG cache entry instead of stdout",
    )
    parser.set_defaults(handler=_cmd_html)


def _add_lint(subparsers: Any) -> None:
    """Wire ``lint``: the deterministic patch checks, CLI only.

    No MCP tool reaches this, like ``validate`` and ``vote`` and for the same
    reason: a patch is write-side input. And no finding fails the command --
    the report says what to look at, the tests say what works.
    """
    parser = subparsers.add_parser(
        "lint",
        help="deterministic hallucination checks over candidate patches (write side)",
        description="Run the deterministic patch checks and print what they found. "
        "Advisories, warnings and coverage gaps are all reported and none of them fails "
        "the command: the exit code is 0 unless the invocation itself was unusable.",
    )
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument(
        "--candidates",
        metavar="PATH",
        help="a patch file, or a directory of them (edits.json or SEARCH/REPLACE text)",
    )
    source.add_argument(
        "--diff",
        metavar="FILE",
        help="a unified diff to check instead (git diff, or a format-patch body); "
        "the checks compare it against --repo as it stands, so --repo must be a checkout "
        "of the diff's BASE, not a tree with the diff already applied",
    )
    _repo_flags(parser)
    parser.set_defaults(handler=_cmd_lint)


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
    here in the CLI -- no MCP tool can reach it -- and refused unless
    ``--allow-repo-test-cmd`` says otherwise. There is still no ``Makefile``
    sniffing, no ``package.json`` scripts lookup and no built-in default.

    ``--allow-test-config-edits`` is that rule's write-side twin: a candidate
    that edits ``conftest.py``, a build file or a CI workflow is refused
    before it is applied, because it would be choosing how it is judged.

    The opt-in is the gate, not the note printed beside it: this CLI is the
    front door any agent can reach over Bash, so "a human saw the command"
    is not a property the code can hold, and the refusal lives in
    :mod:`agentless_mcp.application.validate_service` where every caller
    meets it.
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
        "Falls back to test_cmd in the repository's .agentless-mcp.json, if it has one and "
        "--allow-repo-test-cmd was given",
    )
    parser.add_argument(
        "--allow-repo-test-cmd",
        action="store_true",
        help="allow the test command to come from the analysed repository's "
        ".agentless-mcp.json; without this the fallback is refused, because the "
        "repository would be choosing the command that judges it",
    )
    parser.add_argument(
        "--allow-test-config-edits",
        action="store_true",
        help="allow a candidate patch to edit conftest.py, a build file or a CI workflow; "
        "without this such a candidate is refused before it is applied, because it "
        "would be choosing how it is judged",
    )
    parser.add_argument(
        "--pass-env",
        action="append",
        default=[],
        type=_environment_name,
        metavar="NAME",
        help="pass one additional parent environment variable to test commands; repeatable. "
        "By default a test command inherits only a short per-platform allowlist: "
        "PATH, HOME, LANG and TMPDIR on POSIX, and the names a Windows interpreter "
        "needs to start on Windows",
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
        "a command that hits it is a failure, never a pass; killing a stubborn command "
        f"adds at most {sandbox.TERM_GRACE_SECONDS + sandbox.KILL_REAP_SECONDS:g}s of cleanup",
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=DEFAULT_JOBS,
        help=f"distinct candidate results to run concurrently (default: {DEFAULT_JOBS})",
    )
    parser.add_argument(
        "--run-timeout",
        type=int,
        default=None,
        metavar="SECONDS",
        help="bound the whole run, not one command (default: unbounded). A batch is "
        "at most repeat_baseline + 1 + candidates x 2 commands; exact-result groups and "
        "regression failures reduce it; candidates the deadline never reached are reported "
        "not_evaluated",
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


def _add_guide(subparsers: Any) -> None:
    # No _repo_flags: the guide ships with the package and says nothing about
    # any repository, so --repo, --json and --no-cache would all be lies.
    parser = subparsers.add_parser("guide", help="print the packaged agent usage guide")
    parser.add_argument(
        "--section",
        metavar="NAME",
        default=None,
        help="print one section: a tool name ('refs', 'map', 'communities') or "
        "the heading lowercased and hyphenated. An unknown name lists them all",
    )
    parser.set_defaults(handler=_cmd_guide)


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

    request = MapRequest(
        focus=tuple(args.focus),
        budget=budget,
        max_files=args.max_files,
        granularity=args.granularity,
    )
    if args.granularity == GRANULARITY_BODY:
        body_result = build_body_map(ctx, request, services.maps, services.symbols)
        _emit(
            args,
            ctx,
            services,
            _Answer(
                text=render_body_map(services.maps, body_result),
                payload=body_result.as_dict(),
                items_key="files",
                truncation=envelope.Truncation(
                    shown=len(body_result.bodies.cards),
                    total=body_result.map.candidates,
                    unit="symbols",
                ),
            ),
        )
        return EXIT_OK

    result = services.maps.build(ctx, request)
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
    # The reason a file could not be read goes on stderr, never into the view.
    # Interleaved, it rendered as source: an agent piping `skeleton a.py b.py`
    # into a prompt read "b.py: unreadable: No such file or directory" as the
    # contents of b.py, behind exit 0. The JSON form keeps the per-file error
    # as its own field, which is a structure a reader can tell apart.
    failed = [view for view in views if view.error]
    # `render.overview_block` is the same block the MCP operation renders, so
    # the two doors onto this view cannot drift again. The error argument is
    # empty here on purpose: the reason goes to stderr below, which is the
    # split this function's comment above describes.
    text = "\n".join(
        render.overview_block(view.path, view.language, "", view.text)
        for view in views
        if not view.error
    )
    _emit(args, ctx, services, _Answer(text, {"files": [v.as_dict() for v in views]}, "files"))
    for view in failed:
        # The same wording `slice` uses for the identical FileView.error: the
        # service message already names the file it is about.
        note(f"agentless-mcp: {view.error}")
    # A refused path outranks an unreadable one, and keys on the typed marker
    # rather than on the message: a path outside the root is a usage error
    # however many other files the batch answered, while a file that could not
    # be read is a domain failure. Both beat exit 0 -- the caller named them
    # and they did not resolve.
    if any(view.refused for view in views):
        return EXIT_USAGE
    return EXIT_DOMAIN if failed else EXIT_OK


def _cmd_expand(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    result = services.symbols.expand_symbols(ctx, list(args.ids), limit=args.limit)
    _emit(args, ctx, services, _Answer(render_expansion(result), result.as_dict(), "symbols"))
    # The same split `skeleton` uses, for the same reason. These rows used to
    # ride stdout under the symbol bodies at exit 0, so an agent piping
    # `expand` into a prompt read "unresolved: ... no longer defines X" as
    # source among the sources it asked for. The JSON form keeps them as their
    # own field, which is a structure a reader can tell apart.
    for line in unresolved_lines(result):
        note(f"agentless-mcp: {line}")
    # An id the caller named and did not get back is a failure whether or not
    # the other ids in the batch resolved -- the old rule reported success
    # for a batch that answered one of fifty.
    return EXIT_DOMAIN if result.unresolved else EXIT_OK


def _cmd_slice(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    if args.symbol:
        return _slice_by_symbol(args, ctx, services)

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


def _slice_by_symbol(args: argparse.Namespace, ctx: RepoContext, services: CliServices) -> int:
    """Render the source of whatever symbol one stable id names.

    ``--lines`` belongs to a FILE slice and this path discards it, so the
    combination is refused rather than half-honoured.
    """
    if args.lines:
        return fail("slice takes --lines with FILE, not with --symbol", EXIT_USAGE)
    try:
        parsed = parse_stable_id(args.symbol)
    except ValueError as error:
        return fail(str(error), EXIT_USAGE)

    located = _locate_symbol(services, ctx, parsed, context=args.context)
    _emit(args, ctx, services, _Answer(_render_locations(located), located.as_dict()))
    return EXIT_OK if located.resolution.stable_ids else EXIT_DOMAIN


def _locate_symbol(
    services: CliServices,
    ctx: RepoContext,
    parsed: StableId,
    *,
    context: int,
) -> LocationView:
    """Resolve a stable id by trying each location form its qualname can take.

    An id carries a path and a qualified name but no kind, so the kind has to
    come from the shape of the name and from what the file actually holds.
    Hardcoding ``function:`` here is what made every class, dataclass, enum,
    protocol and constant id -- all of which ``expand``, ``refs`` and
    ``explain`` accept -- answer "no such symbol". The forms are tried one at
    a time rather than in one call because ``class:`` sets the current class
    for the locations after it, and the first that resolves wins.

    When none resolves the caller gets every form that was tried with the
    reason it missed, which is the difference between "this symbol is not a
    function" and "this symbol does not exist".
    """
    attempts: list[LocationView] = []
    for loc in _symbol_locs(parsed.qualname):
        view = services.views.resolve_locations(ctx, parsed.path, [loc], context=context)
        if view.resolution.stable_ids:
            return view
        attempts.append(view)

    last = attempts[-1]
    misses = tuple(entry for view in attempts for entry in view.resolution.unrecognized)
    return replace(last, resolution=replace(last.resolution, unrecognized=misses))


def _symbol_locs(qualname: str) -> tuple[str, ...]:
    """Return the location forms a stable id's qualified name may resolve as.

    A dotted name is a member of something and only the function branch reads
    those; a bare name can be a module-level function, a class of any of the
    class-like kinds, or a module-level constant.
    """
    if "." in qualname:
        return (f"function: {qualname}",)
    return (f"function: {qualname}", f"class: {qualname}", f"variable: {qualname}")


def _cmd_find_symbol(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    result = services.symbols.find_symbol(ctx, args.name, kind=args.kind, limit=args.limit)
    _emit(
        args,
        ctx,
        services,
        _Answer(render_find(result), result.as_dict(), "matches"),
    )
    return EXIT_OK if result.cards else EXIT_DOMAIN


def _cmd_refs(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    result = services.symbols.find_referencing_symbols(
        ctx, args.target, limit=args.limit, shared_callers=args.shared_callers
    )
    text = render_refs(result, shared_callers=args.shared_callers)
    _emit(args, ctx, services, _Answer(text, result.as_dict(), "groups"))
    return EXIT_OK


def _cmd_explain(args: argparse.Namespace, services: CliServices) -> int:
    """Render one symbol's definition site with its tiered fan-out and fan-in."""
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    result = services.graphs.explain(ctx, args.target, limit=args.limit)
    _emit(
        args,
        ctx,
        services,
        _Answer(render.render_explanation(result), result.as_dict(), "fan_out"),
    )
    return EXIT_OK if result.card is not None else EXIT_DOMAIN


def _cmd_path(args: argparse.Namespace, services: CliServices) -> int:
    """Render the shortest resolved path between two symbols.

    An endpoint that does not name anything is a domain failure; a pair with
    no path between them is an answer, and exits 0 like every other
    legitimately empty view.
    """
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE
    result = services.graphs.path(
        ctx,
        args.source,
        args.target,
        PathOptions(
            include_unique=args.include_unique,
            include_ambiguous=args.include_ambiguous,
            max_visited=args.max_visited,
        ),
    )
    _emit(args, ctx, services, _Answer(render.render_path(result), result.as_dict(), "hops"))
    return EXIT_OK if result.endpoints_resolved else EXIT_DOMAIN


def _cmd_cycles(args: argparse.Namespace, services: CliServices) -> int:
    """Render every module-level import cycle; no cycles is a successful answer."""
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    result = services.graphs.cycles(ctx, limit=args.limit)
    _emit(args, ctx, services, _Answer(render.render_cycles(result), result.as_dict(), "cycles"))
    return EXIT_OK


def _cmd_communities(args: argparse.Namespace, services: CliServices) -> int:
    """Render the file communities; an empty repository is a successful answer."""
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE
    result = services.graphs.communities(
        ctx, resolution=args.resolution, limit=args.limit, members=args.members
    )
    _emit(
        args,
        ctx,
        services,
        _Answer(render.render_communities(result), result.as_dict(), "communities"),
    )
    return EXIT_OK


def _cmd_health(args: argparse.Namespace, services: CliServices) -> int:
    """Render the structural-health sections; a clean repository is a successful answer."""
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE

    result = services.graphs.health(ctx, limit=args.limit)
    _emit(args, ctx, services, _Answer(render.render_health(result), result.as_dict()))
    return EXIT_OK


def _cmd_diagram(args: argparse.Namespace, services: CliServices) -> int:
    """Render the module graph as mermaid, or compare it against a committed one."""
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE
    view = services.graphs.diagram(
        ctx,
        DiagramRequest(
            focus=args.focus,
            max_nodes=args.max_nodes,
            max_edges=args.max_edges,
            group_by_communities=args.communities,
            resolution=args.resolution,
        ),
    )
    if view.message:
        return fail(view.message)

    if args.check is not None:
        return _check_diagram(view.text, Path(args.check))
    if args.json:
        # The text form is a document fragment on purpose -- it is pasted into
        # a README -- but the JSON form is read by a machine, and a reader
        # keyed on `document["receipt"]` must not hit a KeyError on the two
        # subcommands whose output is most likely to be cached.
        emit(envelope.wrap_json(ctx, view.as_dict(), counter=services.counter))
    else:
        emit(view.text)

    note("\n".join(envelope.receipt_lines(ctx, summary=_diagram_summary(view))))
    if view.caveat:
        note(f"agentless-mcp: {view.caveat}")
    return EXIT_OK


def _cmd_html(args: argparse.Namespace, services: CliServices) -> int:
    """Render HTML to stdout or one explicitly named XDG-cache file."""
    ctx = _context(args, services)
    if ctx is None:
        return EXIT_USAGE
    cache_name = _html_cache_name(args.cache_file)
    if args.cache_file is not None and cache_name is None:
        return fail(
            f"--cache-file must be a simple .html name of at most {MAX_HTML_CACHE_NAME} characters",
            EXIT_USAGE,
        )

    exported = services.graphs.html(
        ctx,
        max_nodes=args.max_nodes,
        max_edges=args.max_edges,
        resolution=args.resolution,
    )
    if cache_name is None:
        emit(exported.text)
    else:
        target = cache.cache_path(ctx.root).parent / "exports" / cache_name
        try:
            _write_html_export(target, exported.text)
        except OSError as error:
            return fail(f"cannot write HTML export: {error}", EXIT_DOMAIN)
        note(f"agentless-mcp: wrote {target}")

    note("\n".join(envelope.receipt_lines(ctx, summary=_html_summary(exported, args))))
    return EXIT_OK


def _html_summary(exported: HtmlExport, args: argparse.Namespace) -> str:
    """Say what the export left out, and which bound took it.

    One number for two causes was unreadable: `0 modules and 200 edges elided`
    on this repository meant every one of the 200 came from the edge bound and
    none from the node bound, but the line could equally have described the
    reverse -- and only one of those is fixed by raising --max-nodes.
    """
    return (
        f"HTML graph of {exported.nodes} modules and {exported.edges} edges; "
        f"{exported.elided_nodes} modules elided (node bound {args.max_nodes}); "
        f"{exported.edges_without_both_nodes} edges elided with them, "
        f"{exported.edges_over_bound} past the edge bound ({args.max_edges})"
    )


def _html_cache_name(raw: str | None) -> str | None:
    """Parse a cache-only output name; paths never cross this boundary."""
    if raw is None:
        return None
    if len(raw) > MAX_HTML_CACHE_NAME or HTML_CACHE_NAME.fullmatch(raw) is None:
        return None
    return raw


def _write_html_export(target: Path, document: str) -> None:
    """Atomically replace one cache export without following a target symlink."""
    target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            encoding="utf-8",
            dir=target.parent,
            prefix=f".{target.name}.",
            delete=False,
        ) as stream:
            stream.write(document)
            stream.flush()
            os.fsync(stream.fileno())
            temporary = Path(stream.name)
        temporary.chmod(0o600)
        temporary.replace(target)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()


def _cmd_lint(args: argparse.Namespace, services: CliServices) -> int:
    """Run the deterministic patch checks and print what they found.

    Always exit 0 on a run that happened. A finding is something to look at,
    not a verdict, and a lint that failed the command would be a gate nobody
    asked for standing between a candidate and the tests that actually decide.
    """
    ctx = _context(args, services, require_git=False)
    if ctx is None:
        return EXIT_USAGE

    candidates = (
        (load_diff(Path(args.diff)),)
        if args.diff is not None
        else load_candidates(Path(args.candidates))
    )
    report = services.lints.lint(ctx, candidates)
    _emit(args, ctx, services, _Answer(render.render_lint(report), report.as_dict(), "candidates"))
    return EXIT_OK


def _diagram_summary(view: render.DiagramView) -> str:
    """Return the one-line summary a diagram's receipt carries."""
    focus = f" around {view.focus}" if view.focus else ""
    grouped = ", grouped by community" if view.grouped else ""
    return f"diagram of {view.nodes} modules{focus}{grouped}; {view.elided} elided"


def _check_diagram(rendered: str, target: Path) -> int:
    """Compare a fresh render against a committed diagram.

    Byte-exact after the fence is stripped, because "the diagram in the
    repository is the diagram this repository produces" is either true or it
    is not. What the caller gets on a mismatch is the first line that differs
    and the two lengths -- enough to see whether the drift is the tree moving
    or the flags differing, without printing two diagrams.
    """
    committed, reason = _read_caller_file(target)
    if committed is None:
        return fail(f"cannot read {target}: {reason}", EXIT_USAGE)

    stripped = render.strip_fence(committed)
    if stripped == rendered:
        note(f"agentless-mcp: {target} matches the current diagram")
        return EXIT_OK

    note(f"agentless-mcp: {target} has drifted from the current diagram")
    for line in _drift_summary(stripped, rendered):
        note(f"  {line}")
    return EXIT_DOMAIN


def _drift_summary(committed: str, rendered: str) -> list[str]:
    """Return a short description of where two diagrams first disagree."""
    old = committed.split("\n")
    new = rendered.split("\n")
    lines = [f"committed {len(old)} lines, regenerated {len(new)} lines"]
    for number, (before, after) in enumerate(zip(old, new, strict=False), start=1):
        if before != after:
            lines.append(f"first difference at line {number}:")
            lines.append(f"  committed:   {before}")
            lines.append(f"  regenerated: {after}")
            return lines
    lines.append(f"identical for the first {min(len(old), len(new))} lines, then one is longer")
    return lines


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

    from_config = args.test_cmd is None
    if from_config:
        # Printed before the run, not after: this command came out of the
        # repository being judged, and the caller has to see which one it is.
        # The note is not the control -- the service refuses the run unless
        # --allow-repo-test-cmd was given -- it is what makes the refusal,
        # or the opt-in, name a command.
        opted_in = (
            "--allow-repo-test-cmd was given, so it will run"
            if args.allow_repo_test_cmd
            else "pass --test-cmd to name your own, or --allow-repo-test-cmd to run this one"
        )
        note(
            f"agentless-mcp: test command from {ctx.config.path}: {test_cmd}\n"
            f"agentless-mcp: it comes from the repository under analysis; {opted_in}."
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
            run_timeout=args.run_timeout,
            passthrough_env=tuple(dict.fromkeys(args.pass_env)),
            test_cmd_from_repo=from_config,
            allow_repo_test_cmd=args.allow_repo_test_cmd,
            allow_test_config_edits=args.allow_test_config_edits,
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
            # The run has already spent minutes on a baseline and two commands
            # per candidate, and the document is the only record of it. Losing
            # the work as well as the write is the avoidable half of the
            # failure, so the document goes to stdout and the exit code still
            # says the write did not happen.
            fail(f"cannot write {destination}: {error.strerror}; verdicts on stdout", EXIT_USAGE)
            emit(document)
            return EXIT_USAGE
        note(f"agentless-mcp: verdicts written to {destination}")

    note("\n".join(envelope.receipt_lines(ctx, summary=report.summary_line())))
    for warning in report.warnings():
        note(f"agentless-mcp: {warning}")
    return EXIT_OK if report.any_passed else EXIT_DOMAIN


def _cmd_vote(args: argparse.Namespace, services: CliServices) -> int:
    """Rank a verdicts document by equivalence cluster.

    Exit ``0`` only for a ranking a caller can act on. A run whose baseline
    never went green ranked nothing, and a run where no candidate applied
    cleanly has no tier to rank in; both told the caller something, and
    neither is a winner.
    """
    _ = services
    source = Path(args.verdicts)
    text, reason = _read_caller_file(source)
    if text is None:
        return fail(f"cannot read {source}: {reason}", EXIT_USAGE)

    loaded = load_verdicts(text)
    report = vote.rank(loaded.candidates, repro_valid=loaded.repro_valid)

    if args.json:
        emit(json.dumps(report.as_dict(), indent=2))
    else:
        emit(report.text())

    note(f"# agentless-mcp vote over {source}")
    note(f"# test-cmd: {loaded.test_cmd}")
    note(f"# {report.summary_line()}")
    verified = loaded.baseline is BaselineStatus.OK
    if not verified:
        note(
            "agentless-mcp: this ranking comes from an UNVERIFIED run -- the test command "
            "did not pass on unpatched HEAD, so no candidate was evaluated."
        )
    return EXIT_OK if verified and report.tier != vote.TIER_NONE else EXIT_DOMAIN


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
    """Build or refresh the tag cache for one repository.

    A per-file error exits 1: the index was only partially built, and a
    ``warmup && index`` gate that saw 0 would proceed with every later query
    silently under-covering the errored files. The report still prints
    whole -- partial is an answer, but it is not a success.

    A known language whose grammar is not warmed is a warning, not an error:
    the file is recorded with its digest and listed as skipped, and the exit
    code stays 0. A fresh install warms only tier 1, so failing ``index``
    over a repository's own yaml and toml would make the exit-code gate
    permanent noise instead of a signal.
    """
    ctx = _resolve(args, require_git=False)
    if ctx is None:
        return EXIT_USAGE

    # `build_index` opens and writes a SQLite database, so it raises
    # `sqlite3.Error` and `OSError` -- neither of which is an `AgentlessError`,
    # so neither was caught by the handler in `run`. A full disk or a
    # read-only cache directory ended this command in a raw traceback, which
    # also puts an absolute local path on stderr. Converted to the package's
    # own refusal, naming the repository the caller would have to act on --
    # the repository rather than the database, because the cache path is an
    # opaque hash directory derived from it and the root is what an operator
    # can do something about.
    try:
        report = cache.build_index(
            ctx.root,
            services.extractor,
            tree_oid=ctx.tree_oid,
            head_sha=ctx.head_sha,
            force=args.force,
        )
    except (sqlite3.Error, OSError) as error:
        message = f"cannot build the tag cache for {ctx.root}: {error}"
        raise OperationFailed(message) from error
    if args.json:
        emit(envelope.wrap_json(ctx, report.as_dict(), counter=services.counter))
        return EXIT_OK if report.errors == 0 else EXIT_DOMAIN

    lines = [report.summary_line()]
    lines.extend(
        f"  error: {failure.path}: {failure.reason}"
        for failure in report.failures[:INDEX_FAILURE_LINES]
    )
    if report.errors > INDEX_FAILURE_LINES:
        lines.append(f"  ... {report.errors - INDEX_FAILURE_LINES} more errors not listed")
    lines.extend(
        f"  warning: {skip.path}: {skip.reason}"
        for skip in report.skipped_files[:INDEX_FAILURE_LINES]
    )
    if report.skipped > INDEX_FAILURE_LINES:
        lines.append(f"  ... {report.skipped - INDEX_FAILURE_LINES} more warnings not listed")
    emit("\n".join(lines))
    return EXIT_OK if report.errors == 0 else EXIT_DOMAIN


def _cmd_capabilities(args: argparse.Namespace, services: CliServices) -> int:
    ctx = _context(args, services, require_git=False)
    if ctx is None:
        return EXIT_USAGE

    report = build_capability_report(ctx, services.extractor)
    _emit(
        args,
        ctx,
        services,
        _Answer(render_capability_report(report), report.as_dict(), "languages"),
    )
    return EXIT_OK


def _cmd_guide(args: argparse.Namespace, services: CliServices) -> int:
    """Print the packaged guide, or one section of it.

    No receipt and no ``_emit``: those describe a repository, and this answer
    describes the tool. A guide missing from the package raises rather than
    printing nothing, because an empty guide reads as "there is nothing to say"
    when it means the install is broken.
    """
    _ = services
    if args.section is None:
        emit(guide.guide_text())
        return EXIT_OK

    text = guide.section_text(args.section)
    if text is None:
        names = ", ".join(guide.section_names())
        return fail(f"no guide section named {args.section!r}. Sections are: {names}", EXIT_USAGE)

    emit(text)
    return EXIT_OK


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _read_caller_file(path: Path) -> tuple[str | None, str]:
    """Read a file the caller named, or say why it could not be read.

    Read as given rather than through :func:`util.fslimits.read_bounded`,
    which the repository-content readers use: that one applies a size cap,
    refuses a symlink and substitutes replacement characters for undecodable
    bytes. All three are right for a file a traversal discovered and wrong
    for one a person typed on a command line.

    What was missing is the decode failure. ``UnicodeDecodeError`` is a
    ``ValueError``, so an ``except OSError`` around ``read_text`` let a
    latin-1 patch file or a mistyped binary path out as a raw traceback --
    which also puts an absolute local path on stderr. The MCP adapter's two
    readers already catch both; this is the same handling on the other door.
    """
    try:
        return path.read_text(encoding="utf-8"), ""
    except OSError as error:
        return None, error.strerror or str(error)
    except UnicodeDecodeError as error:
        return None, f"not valid UTF-8: byte {error.start} is {error.reason}"


def _patch_text(args: argparse.Namespace) -> str | None:
    """Read the patch text from ``--file`` or stdin, or report why not.

    The patch is the caller's own input rather than repository content, so it
    is read as given: containment applies to the paths *inside* it, which the
    service checks against the repository root before anything is opened.
    """
    if args.file is None or args.file == "-":
        return sys.stdin.read()

    source = Path(args.file)
    text, reason = _read_caller_file(source)
    if text is None:
        fail(f"cannot read {source}: {reason}", EXIT_USAGE)
    return text


def _patch_call(
    args: argparse.Namespace, services: CliServices
) -> tuple[RepoContext, tuple[Edit, ...]] | None:
    """Resolve the repository and load the edits one write subcommand acts on.

    A text with any malformed block is refused whole. Noting the errors and
    handing the surviving edits to the write side is the failure mode
    :class:`~agentless_mcp.core.patches.ParseResult` exists to prevent: a
    truncated generation applied its first half and exited 0. ``validate``
    already refuses on the same condition; this is the same rule, in the one
    place the other two write commands come through.
    """
    _ = services
    ctx = _resolve(args, require_git=False)
    if ctx is None:
        return None

    text = _patch_text(args)
    if text is None:
        return None

    parsed = load_edits(text)
    if parsed.errors:
        blocks = "\n".join(
            f"  block {error.index} ({error.path or 'no path'}): {error.reason}"
            for error in parsed.errors
        )
        message = (
            f"{len(parsed.errors)} of {len(parsed.errors) + len(parsed.edits)} blocks "
            f"did not parse, so none of them was applied:\n{blocks}"
        )
        raise OperationFailed(message)
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
    note("\n".join(envelope.receipt_lines(ctx, summary=summary)))
    for outcome in result.failures:
        edit = outcome.edit
        note(f"  {outcome.status.value}: {edit.path} block {edit.index}: {outcome.reason}")


def _resolve(args: argparse.Namespace, *, require_git: bool) -> RepoContext | None:
    """Resolve the repository this invocation is about, or report why not.

    ``allowlist=None`` throughout: in the CLI the root comes from the caller's
    own cwd or their own ``--repo``, which is exactly as trusted as the process
    itself. The allowlist exists for the server, where it is not.

    The degradation warning fires at the one exit rather than per branch.
    Warning only on the cwd-git-root branch meant ``--repo`` -- the dominant
    agent invocation -- never got it, so a caller piping stdout into a prompt
    learned nothing about a degraded repository until after they had used the
    answer.
    """
    if args.repo is not None:
        ctx = resolve_repo(args.repo, None)
    else:
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
            ctx = resolve_repo(cwd, None)
        else:
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
    resolved = replace(ctx, symbols=source)
    if services.resources is not None:
        services.resources.callback(resolved.close)
    return resolved


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
    """Emit one answer in whichever form the caller asked for.

    What the service left out is reported in both forms. The text form gets
    the envelope's truncation note; the JSON form gets the same three numbers
    as a field, because a reader that cannot see the note has no other way to
    learn the answer is partial.
    """
    if args.json:
        emit(
            envelope.wrap_json(
                ctx,
                _with_truncation(answer),
                counter=services.counter,
                items_key=answer.items_key,
            )
        )
    else:
        emit(
            envelope.wrap(ctx, answer.text, counter=services.counter, truncation=answer.truncation)
        )


def _with_truncation(answer: _Answer) -> dict[str, Any]:
    """Return the payload, carrying what the service left out when it did."""
    cut = answer.truncation
    if cut is None or cut.shown >= cut.total:
        return answer.payload
    return {
        **answer.payload,
        "truncation": {"shown": cut.shown, "total": cut.total, "unit": cut.unit},
    }


def _render_locations(view: LocationView) -> str:
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
    if args.run_timeout is not None and args.run_timeout <= 0:
        return "--run-timeout takes a positive number of seconds"
    if len(args.pass_env) > MAX_PASSTHROUGH_ENV:
        return f"--pass-env may be repeated at most {MAX_PASSTHROUGH_ENV} times"
    return ""


def _environment_name(raw: str) -> str:
    """Parse one bounded environment variable name from the CLI."""
    if not ENVIRONMENT_NAME.fullmatch(raw):
        message = f"environment variable names must match {ENVIRONMENT_NAME.pattern!r}"
        raise argparse.ArgumentTypeError(message)
    return raw


def _map_budget(raw: str | None, ctx: RepoContext) -> tuple[int | None, str]:
    """Resolve ``--budget``: a number, ``auto``, or the reason it is neither.

    ``None`` for the budget means auto-size, which is why this cannot go
    through :func:`projectconfig.resolve`: "the caller said auto" and "nobody
    said anything" are the same value there and must not be the same decision
    here.
    """
    if raw is None:
        return (None if ctx.config.map_budget is None else ctx.config.map_budget), ""
    if raw == AUTO_BUDGET:
        return None, ""

    parsed = _positive_int(raw)
    if parsed is None:
        return None, f"--budget takes a positive integer or '{AUTO_BUDGET}'"
    return parsed, ""
