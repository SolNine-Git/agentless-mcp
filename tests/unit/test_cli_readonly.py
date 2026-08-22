"""Read commands leave the target repository unmodified.

The write side already gates this invariant (test_sandbox.py compares
porcelain status and HEAD around worktree use); the read side was only true
by construction. This pins it: every read subcommand runs against a
committed fixture repository, and the repository's observable state --
porcelain status, HEAD, and a content hash of the working tree -- must be
byte-identical afterwards.

``index`` is included deliberately. It is the one read command that writes,
and its writes must land in the XDG cache -- pointed at tmp by the autouse
``isolated_cache_home`` fixture -- never inside the repository. That
placement is the behavior this test pins.
"""

import hashlib
import subprocess

import pytest

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

# Every read subcommand the CLI publishes, each with the smallest well-typed
# argument set that succeeds against the fixture repository. Exit codes are
# asserted OK so a refactor that broke a command could not pass this test by
# erroring out before it ever touched the tree.
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
}


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


class TestReadCommandsAreReadOnly:
    @pytest.mark.parametrize("argv", list(READ_COMMANDS.values()), ids=list(READ_COMMANDS))
    def test_the_repository_is_byte_identical_after_the_command(self, services, read_repo, argv):
        before = _snapshot(read_repo)

        assert run([*argv, "--repo", str(read_repo)], services) == EXIT_OK

        assert _snapshot(read_repo) == before

        if argv[0] == "index":
            # The one write the command makes must exist -- otherwise this
            # case passes vacuously -- and must live outside the repository.
            database = cache.cache_path(read_repo.resolve())
            assert database.exists()
            assert read_repo.resolve() not in database.parents
