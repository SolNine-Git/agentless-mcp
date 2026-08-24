"""Read commands leave the target repository unmodified.

The write side already gates this invariant (test_sandbox.py compares
porcelain status and HEAD around worktree use); the read side was only true
by construction. This pins it: every read subcommand runs against a
committed fixture repository, and the repository's observable state --
porcelain status, HEAD, and a content hash of the working tree -- must be
byte-identical afterwards.

``index`` and ``html --cache-file`` are included deliberately. They are the
read commands that write, and their writes must land in the XDG cache --
pointed at tmp by the autouse ``isolated_cache_home`` fixture -- never inside
the repository. That placement is the behavior this test pins.

Which commands run here is derived from the parser rather than listed by hand.
A name list could not fail closed: a write-capable subcommand added tomorrow
would simply not appear in it, and the suite would stay green while the new
command went unguarded.
"""

import argparse
import hashlib
import subprocess

import pytest

from agentless_mcp.adapters.cli import main as cli
from agentless_mcp.adapters.cli.formatting import EXIT_OK
from agentless_mcp.adapters.cli.main import CliServices, run
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.lint_service import LintService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.validate_service import ValidateService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import cache

SOURCE = """\
def quote(sku):
    return 1


class PriceBook:
    def cost_of(self, sku):
        return quote(sku)
"""

CALLER = """\
from core import quote


def run_billing(items):
    return sum(quote(item) for item in items)
"""

# What each repository-scoped subcommand is allowed to write. The gate below
# asserts this table names the parser's repository-scoped subcommands exactly,
# so one added tomorrow fails the suite until someone classifies it. Keying on
# a hand-written list of read commands instead left an unclassified new command
# unguarded by default and the suite green -- a guard that cannot see the thing
# it is meant to protect. Which commands are repository-scoped is read off the
# parser (a subcommand that declares --repo, at any nesting depth) rather than
# listed here, so `vote` and `guide` drop out because they take no repository,
# not because someone remembered to leave them out.
READS_ONLY = "read"  # writes nothing anywhere
WRITES_CACHE = "writes-cache"  # writes only under the XDG cache
WRITES_REPO = "writes-repo"  # may modify the repository it is pointed at

COMMAND_WRITES = {
    "map": READS_ONLY,
    "tree": READS_ONLY,
    "skeleton": READS_ONLY,
    "expand": READS_ONLY,
    "slice": READS_ONLY,
    "find-symbol": READS_ONLY,
    "refs": READS_ONLY,
    "explain": READS_ONLY,
    "path": READS_ONLY,
    "cycles": READS_ONLY,
    "communities": READS_ONLY,
    "diagram": READS_ONLY,
    "resolve-locs": READS_ONLY,
    "capabilities": READS_ONLY,
    "lint": READS_ONLY,
    # `index` and `html --cache-file` both land a file under the XDG cache;
    # the placement, not the absence of a write, is what this suite pins.
    "index": WRITES_CACHE,
    "html": WRITES_CACHE,
    # The write side. `patch apply` edits a worktree and `validate` runs a
    # test command against one; test_sandbox.py owns their invariant.
    "patch": WRITES_REPO,
    "validate": WRITES_REPO,
}

# The smallest well-typed argument set that succeeds against the fixture, for
# every command classified read or writes-cache. Exit codes are asserted OK so
# a refactor that broke a command could not pass this test by erroring out
# before it ever touched the tree. ``DIFF_FILE`` is replaced with a path
# outside the repository, because writing the input inside it would move the
# snapshot this suite compares.
DIFF_FILE = "<diff-file>"

READ_COMMANDS = {
    "map": ["map"],
    "tree": ["tree"],
    "skeleton": ["skeleton", "core.py"],
    "expand": ["expand", "py:core.py::quote"],
    "slice": ["slice", "core.py", "--lines", "1:2"],
    "find-symbol": ["find-symbol", "quote"],
    "refs": ["refs", "quote"],
    "explain": ["explain", "quote"],
    "path": ["path", "caller.py", "core.py"],
    "cycles": ["cycles"],
    "communities": ["communities"],
    "diagram": ["diagram"],
    "resolve-locs": ["resolve-locs", "core.py", "--loc", "function:quote"],
    "capabilities": ["capabilities"],
    "index": ["index"],
    # The two the hand-written name list left out. `html --cache-file` is the
    # one that matters: it writes, and the write must land where `index` puts
    # its own -- the exact behaviour this suite exists to pin.
    "html": ["html", "--cache-file", "graph.html"],
    "lint": ["lint", "--diff", DIFF_FILE],
}


def _subcommands(parser):
    """Every top-level subcommand the parser publishes, name to subparser."""
    found = {}
    for action in parser._actions:
        if isinstance(action, argparse._SubParsersAction):
            found.update(action.choices)
    return found


def _declares_repo(parser):
    """Does this subparser, or any subparser under it, take ``--repo``?"""
    if any("--repo" in action.option_strings for action in parser._actions):
        return True
    return any(_declares_repo(nested) for nested in _subcommands(parser).values())


def _repo_scoped_commands():
    """The published subcommands that are about a repository at all."""
    published = _subcommands(cli.build_parser())
    return {name for name, sub in published.items() if _declares_repo(sub)}


@pytest.fixture
def services(extractor, counter):
    """The same wiring bootstrap builds, without the console-script layer."""
    return CliServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor, counter),
        graphs=GraphService(extractor),
        patches=PatchService(extractor),
        validates=ValidateService(PatchService(extractor)),
        lints=LintService(extractor),
        counter=counter,
        extractor=extractor,
    )


@pytest.fixture
def read_repo(make_git_repo):
    """A committed two-file repository, so refs and path have an edge to walk."""
    return make_git_repo({"core.py": SOURCE, "caller.py": CALLER}, name="readonly")


def _git_out(root, *arguments):
    """Run one read-only git command in ``root`` and return its stdout."""
    return subprocess.run(
        ["git", "-C", str(root), *arguments],
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    ).stdout


def _snapshot(root):
    """The repository's observable state: git status, HEAD, and tree content.

    The content digest covers every file outside .git -- path and bytes --
    because porcelain alone would miss a write to an ignored file. .git
    itself is excluded from the digest: ``git status`` legitimately
    refreshes its own stat cache, and HEAD plus porcelain already pin the
    git-visible state.
    """
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if ".git" in relative.parts or not path.is_file():
            continue
        digest.update(str(relative).encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return (
        _git_out(root, "status", "--porcelain"),
        _git_out(root, "rev-parse", "HEAD"),
        digest.hexdigest(),
    )


class TestEveryRepositoryCommandIsClassified:
    """The gate: the table and the parser must name the same commands.

    An unclassified subcommand fails here rather than skipping the read-only
    check, which is the difference between a guard on the invariant and a
    guard on a list someone maintains.
    """

    def test_the_classification_covers_the_parser_exactly(self):
        assert set(COMMAND_WRITES) == _repo_scoped_commands()

    def test_every_command_that_must_hold_still_is_driven(self):
        must_hold_still = {name for name, kind in COMMAND_WRITES.items() if kind != WRITES_REPO}
        assert set(READ_COMMANDS) == must_hold_still

    def test_the_commands_that_take_no_repository_are_not_classified(self):
        # Derived, not asserted by name: these three declare no --repo, so
        # there is no target repository for them to leave alone.
        assert not {"vote", "guide", "warmup"} & set(COMMAND_WRITES)


class TestReadCommandsAreReadOnly:
    @pytest.mark.parametrize("command", list(READ_COMMANDS), ids=list(READ_COMMANDS))
    def test_the_repository_is_byte_identical_after_the_command(
        self, services, read_repo, tmp_path, command
    ):
        diff_file = tmp_path / "empty.diff"
        diff_file.write_text("", encoding="utf-8")
        argv = [
            diff_file.as_posix() if part is DIFF_FILE else part for part in READ_COMMANDS[command]
        ]
        before = _snapshot(read_repo)

        assert run([*argv, "--repo", str(read_repo)], services) == EXIT_OK

        assert _snapshot(read_repo) == before

        if COMMAND_WRITES[command] == WRITES_CACHE:
            # The write each of these makes must exist -- otherwise the case
            # passes vacuously -- and must live outside the repository.
            entry = cache.cache_path(read_repo.resolve())
            written = entry if command == "index" else entry.parent / "exports" / "graph.html"
            assert written.exists()
            assert read_repo.resolve() not in written.parents
