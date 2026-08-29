"""Shared pytest fixtures.

Grammar access is the one thing in this suite that could reach the network,
so it happens exactly once, here, at session scope. Parsing itself is never
mocked: a test that stubs the parser proves nothing about a tool whose whole
job is parsing.

If the machine's language-pack cache is cold, this fixture performs one real
warmup (a single platform bundle download) and every later run is offline.
"""

import os
import shlex
import subprocess
import sys
from pathlib import Path

import pytest
import tree_sitter_language_pack as pack

from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core import cache, grammars, selfrestart
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util import cachedir
from agentless_mcp.util.tokens import Chars4Counter

# Git exports GIT_DIR to hook processes, and GIT_DIR overrides the -C every
# fixture git call is given: under a pre-push hook, a fixture's `git init`
# reinitializes the *enclosing* repository as bare and `git add -A` rewrites
# its index. Scrubbing the family here, before any test runs a subprocess,
# keeps every git the suite spawns inside its tmp_path -- the same invariant
# core.gitinfo.subprocess_env enforces for the package's own calls.
for _name in [name for name in os.environ if name.startswith("GIT_")]:
    del os.environ[_name]

# The languages the suite actually parses. Kept short on purpose: warming the
# full tier-1 set would slow a cold run down for no coverage gain.
TEST_LANGUAGES = ("python", "javascript", "typescript", "go")


@pytest.fixture(scope="session", autouse=True)
def warm_grammars() -> grammars.WarmupReport:
    """Warm the grammars the suite needs, once, before any test parses.

    The language pack finds its grammars under ``XDG_CACHE_HOME`` and so does
    the tag cache, and every test moves that variable to a throwaway directory
    (see :func:`isolated_cache_home`). Pinning the pack to the real directory
    here, before the first move, is what keeps the two isolations independent:
    grammars stay where they were downloaded, tag caches never leave tmp.
    """
    # setdefault, deliberately: CI points the pack at a cached directory
    # through this same variable, and honouring that is the point.
    os.environ.setdefault(grammars.ENV_CACHE_DIR, pack.cache_dir())
    # Grammar access happens exactly once, here: without this, every run()
    # call and every spawned console script would start its own background
    # warm of the full language set into the shared cache. Tests that cover
    # the auto-warm itself clear the variable and stub the warm seam.
    #
    # Assigned, not setdefault: these three are kill switches, and a suite
    # that reads an ambient value for them is not hermetic. Subprocess tests
    # inherit this environment, so an operator with AGENTLESS_MCP_NO_AUTO_INDEX=0
    # exported would silently arm the background indexer for the whole run and
    # change what the suite proves.
    os.environ[grammars.ENV_NO_AUTO_WARM] = "1"
    os.environ[cache.ENV_NO_AUTO_INDEX] = "1"
    os.environ[selfrestart.ENV_NO_AUTO_RESTART] = "1"
    # The cache ceiling is a kill switch too: an operator with a small
    # AGENTLESS_MCP_MAX_CACHE_BYTES exported would have every index-building
    # test sweep its neighbours' caches and change what the suite proves.
    # "0" disables the sweep; the eviction tests set their own value.
    os.environ[cache.ENV_MAX_CACHE_BYTES] = "0"
    report = grammars.warmup(TEST_LANGUAGES)
    if report.degraded:
        details = ", ".join(f"{cap.name}: {cap.detail}" for cap in report.degraded)
        pytest.fail(f"grammar warmup degraded: {details}")
    return report


@pytest.fixture(autouse=True)
def isolated_cache_home(tmp_path_factory, monkeypatch):
    """Give every test its own XDG cache home.

    Autouse and unconditional: a test that wrote a tag cache into the
    developer's real ``~/.cache`` would leave residue that the next run reads
    back as an index, which is the definition of a non-hermetic suite. Tests
    that care about the location call ``cache.cache_path(root)``, which
    resolves through this same variable.
    """
    home = tmp_path_factory.mktemp("xdg-cache")
    monkeypatch.setenv(cachedir.ENV_CACHE_HOME, str(home))
    return home


@pytest.fixture
def fixtures_dir():
    """Path to the committed fixture repositories."""
    return Path(__file__).parent / "characterization" / "fixtures"


@pytest.fixture
def extractor():
    """One extractor per test, as the composition root builds one per process."""
    return TreeSitterExtractor()


@pytest.fixture
def counter():
    """The default token estimator, which the budget tests pin."""
    return Chars4Counter()


@pytest.fixture
def pinned_context():
    """A factory for RepoContexts with git state fixed.

    Golden outputs carry a receipt, and a receipt carries the repository's
    HEAD and dirty count. Reading those from whatever repository the tests
    happen to run inside would make every golden depend on the working tree,
    which is the opposite of a characterization test.

    A fixture rather than a plain helper on purpose: an unrelated third-party
    distribution ships a top-level ``tests`` package into site-packages, so
    ``from tests.conftest import ...`` resolves to somebody else's module.
    """

    def build(root, *, head="0000000f", tree="1111111f", dirty=0, note=""):
        return RepoContext(root=root, head_sha=head, tree_oid=tree, dirty_count=dirty, note=note)

    return build


@pytest.fixture
def make_git_repo(tmp_path):
    """Build a throwaway git repository with a pinned identity and one commit.

    Identity is passed per invocation rather than read from the machine's git
    config: a suite that commits as whoever is logged in is not hermetic, and
    on a machine with no user.email configured it does not run at all.
    """

    def build(files, name="repo"):
        root = tmp_path / name
        root.mkdir(parents=True, exist_ok=True)
        for relative, content in files.items():
            target = root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding="utf-8")

        _git(root, "init", "-b", "main")
        _git(root, "add", "-A")
        _git(
            root,
            "-c",
            "user.email=tests@example.invalid",
            "-c",
            "user.name=agentless-mcp tests",
            "commit",
            "-m",
            "fixture",
        )
        return root

    return build


@pytest.fixture
def commit_all():
    """A second commit in a fixture repository, with the pinned test identity.

    Beside :func:`make_git_repo` because it is the same hermeticity rule
    applied to the commits after the first: identity per invocation, never
    the machine's git config.
    """

    def commit(root, message):
        _git(
            root,
            "-c",
            "user.email=tests@example.invalid",
            "-c",
            "user.name=agentless-mcp tests",
            "commit",
            "-am",
            message,
        )

    return commit


# ---------------------------------------------------------------------------
# The seeded-bug fixture repository the validate/vote suites work against
# ---------------------------------------------------------------------------

# `add` returns a - b where it should return a + b. Everything the validation
# phase is tested on is built from this one seeded sign error.
BUGGY_APP = "def add(a, b):\n    return a - b\n"

# The regression script passes on the buggy code AND on every correct fix,
# which is what a regression suite is: it says "you broke something", not
# "you fixed something". `a * b` fails it, which is how the bad candidate is
# caught.
REGRESSION_SCRIPT = """\
from app import add

assert add(1, 0) == 1, f"add(1, 0) == {add(1, 0)}"
print("regression ok")
"""

# The reproduction script FAILS on the buggy code and passes once it is
# fixed. That polarity is the whole contract: a reproduction test that passes
# on the baseline is reported does_not_reproduce and excluded.
REPRO_SCRIPT = """\
from app import add

assert add(2, 3) == 5, f"add(2, 3) == {add(2, 3)}"
print("reproduction ok")
"""

SEEDED_BUG_FILES = {
    "app.py": BUGGY_APP,
    "check_regression.py": REGRESSION_SCRIPT,
    "check_repro.py": REPRO_SCRIPT,
}


@pytest.fixture
def python_cmd():
    """Build a command string that runs one script with *this* interpreter.

    ``sys.executable`` rather than the string ``python``: a suite that depends
    on what ``python`` resolves to on PATH is a suite that answers differently
    on a machine where it resolves to something else, or not at all.
    """

    def build(script, *arguments):
        parts = [sys.executable, script, *arguments]
        return " ".join(shlex.quote(part) for part in parts)

    return build


@pytest.fixture
def seeded_bug_repo(make_git_repo):
    """A committed git repository carrying the seeded bug and both scripts."""

    def build(name="seeded", overrides=None):
        files = dict(SEEDED_BUG_FILES)
        files.update(overrides or {})
        return make_git_repo(files, name=name)

    return build


@pytest.fixture
def candidates_dir(tmp_path):
    """Write a mapping of filename -> patch text into a candidates directory."""

    def build(patches, name="candidates"):
        directory = tmp_path / name
        directory.mkdir(parents=True, exist_ok=True)
        for filename, text in patches.items():
            (directory / filename).write_text(text, encoding="utf-8")
        return directory

    return build


def _git(root, *arguments):
    """Run one git command in ``root``, failing loudly."""
    subprocess.run(
        ["git", "-C", str(root), *arguments],
        check=True,
        capture_output=True,
        timeout=30,
    )
