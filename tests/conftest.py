"""Shared pytest fixtures.

Grammar access is the one thing in this suite that could reach the network,
so it happens exactly once, here, at session scope. Parsing itself is never
mocked: a test that stubs the parser proves nothing about a tool whose whole
job is parsing.

If the machine's language-pack cache is cold, this fixture performs one real
warmup (a single platform bundle download) and every later run is offline.
"""

import os
import subprocess
from pathlib import Path

import pytest
import tree_sitter_language_pack as pack

from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core import cache, grammars
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.tokens import Chars4Counter

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
    os.environ.setdefault(grammars.ENV_CACHE_DIR, pack.cache_dir())
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
    monkeypatch.setenv(cache.ENV_CACHE_HOME, str(home))
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


def _git(root, *arguments):
    """Run one git command in ``root``, failing loudly."""
    subprocess.run(
        ["git", "-C", str(root), *arguments],
        check=True,
        capture_output=True,
        timeout=30,
    )
