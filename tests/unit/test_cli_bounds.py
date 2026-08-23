"""Every numeric CLI option at 0, at -1, and above its cap.

Nothing owns numeric bounds on this surface. `application/symbol_service`
carries a `_check_limit`, `adapters/cli/main` carries two more rules written
by hand, `GraphService` carries none, and the MCP adapter carries a
`Field(ge=, le=)` on parameters the CLI accepts bare. The result is that one
number means four things depending on which command reads it.

This module pins what each option does today, before stage 4 gives the
services one owner. The exit-code table is deliberately exhaustive rather
than representative: the value of a characterization test is that the diff
after the fix shows every cell that moved, and a sampled table hides the
cells nobody thought to sample.

Three of these are worse than inconsistent. `cycles`, `map` and `communities`
answer a bounded question by reporting that the repository is empty, so a
caller who passes zero is told a fact about the repository rather than about
the bound. Those get their own tests below the table.
"""

from __future__ import annotations

import pytest

from agentless_mcp.adapters.cli.formatting import EXIT_DOMAIN, EXIT_OK, EXIT_USAGE
from agentless_mcp.adapters.cli.main import CliServices, run
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.lint_service import LintService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.validate_service import ValidateService
from agentless_mcp.application.view_service import ViewService

SOURCE = """\
RATE = 3


def quote(sku):
    return RATE


class Book:
    def price(self):
        return quote(1)
"""

CALLER = """\
from core import quote


def ask():
    return quote(2)
"""

# Two files that import each other, so `cycles` has something true to report.
CYCLE = {
    "x.py": "import y\n\n\ndef in_x():\n    return 1\n",
    "y.py": "import x\n\n\ndef in_y():\n    return 2\n",
}

ABOVE_ANY_CAP = "1000000000"

# The positional arguments each command needs before its numeric option can
# be reached at all.
LEADING = {
    "expand": ("py:core.py::quote",),
    "explain": ("quote",),
    "find-symbol": ("quote",),
    "path": ("caller.py", "core.py"),
    "refs": ("quote",),
    "resolve-locs": ("core.py", "--loc", "function:quote"),
    "slice": ("core.py",),
}

# Measured on the fixture below. EXIT_OK means the value was honoured or
# ignored; EXIT_USAGE means an adapter-local rule refused it; EXIT_DOMAIN
# means a service refused it. Read a row across and the divergence is the
# finding: `--limit -1` is a usage error on communities, a domain error on
# refs, and a clean success on cycles.
BOUNDARY_EXITS: dict[tuple[str, str], tuple[int, int, int]] = {
    #                                      0            -1         above cap
    ("communities", "--resolution"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_OK),
    ("communities", "--limit"): (EXIT_OK, EXIT_USAGE, EXIT_OK),
    ("communities", "--members"): (EXIT_OK, EXIT_USAGE, EXIT_OK),
    ("cycles", "--limit"): (EXIT_OK, EXIT_OK, EXIT_OK),
    ("diagram", "--max-nodes"): (EXIT_USAGE, EXIT_USAGE, EXIT_OK),
    ("diagram", "--resolution"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_OK),
    ("expand", "--limit"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_OK),
    ("explain", "--limit"): (EXIT_OK, EXIT_OK, EXIT_OK),
    ("find-symbol", "--limit"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_OK),
    ("html", "--max-nodes"): (EXIT_USAGE, EXIT_USAGE, EXIT_USAGE),
    ("html", "--max-edges"): (EXIT_OK, EXIT_USAGE, EXIT_USAGE),
    ("html", "--resolution"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_OK),
    ("map", "--max-files"): (EXIT_OK, EXIT_OK, EXIT_OK),
    ("path", "--max-visited"): (EXIT_USAGE, EXIT_USAGE, EXIT_OK),
    ("refs", "--limit"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_OK),
    ("resolve-locs", "--context"): (EXIT_OK, EXIT_DOMAIN, EXIT_OK),
    ("slice", "--context"): (EXIT_OK, EXIT_DOMAIN, EXIT_OK),
    ("tree", "--depth"): (EXIT_OK, EXIT_OK, EXIT_OK),
    ("tree", "--max-entries"): (EXIT_OK, EXIT_OK, EXIT_OK),
}

CASES = [
    (command, option, value, expected[position])
    for (command, option), expected in sorted(BOUNDARY_EXITS.items())
    for position, value in enumerate(("0", "-1", ABOVE_ANY_CAP))
]


@pytest.fixture
def services(extractor, counter):
    """The CLI's own wiring, local to this module.

    Five suites already build a fixture named ``services`` holding a
    different type, so this one stays local too rather than becoming a
    global name that means whichever flavour the reader last saw.
    """
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
def repo(tmp_path):
    """Two files, one importing the other, with no git required."""
    (tmp_path / "core.py").write_text(SOURCE, encoding="utf-8")
    (tmp_path / "caller.py").write_text(CALLER, encoding="utf-8")
    return tmp_path


def dispatch(services, root, command, *arguments):
    """Run one subcommand in process and return its exit code."""
    return run([command, *LEADING.get(command, ()), *arguments, "--repo", str(root)], services)


class TestBoundaryExitCodes:
    @pytest.mark.parametrize(
        "case",
        CASES,
        ids=[f"{command}{option}={value}" for command, option, value, _ in CASES],
    )
    def test_each_option_at_each_boundary(self, services, repo, case):
        command, option, value, expected = case
        assert dispatch(services, repo, command, option, value) == expected

    def test_three_refusal_vocabularies_are_in_use_at_once(self, services, repo, capsys):
        # Not three phrasings of one rule: three rules, in three layers, that
        # never learned about each other. Stage 4b is what gives them one.
        dispatch(services, repo, "html", "--max-nodes", "0")
        adapter_range = capsys.readouterr().err

        dispatch(services, repo, "communities", "--limit", "-1")
        adapter_sign = capsys.readouterr().err

        dispatch(services, repo, "refs", "--limit", "0")
        service_rule = capsys.readouterr().err

        assert "--max-nodes takes an integer from 1 through 1000" in adapter_range
        assert "--limit and --members take non-negative integers" in adapter_sign
        assert "limit must be at least 1, got 0" in service_rule


class TestZeroIsAnsweredAsEmptiness:
    """A bound of zero must describe the bound, never the repository."""

    def test_map_names_the_budget_rather_than_blaming_the_repository(self, services, repo, capsys):
        """`render_map` is handed rows and nothing else, so it says only that.

        It used to answer "nothing in this repository parsed into symbols",
        which is a claim about the repository the rows cannot support:
        `MapService._pack` also returns zero when the first candidate exceeds
        the budget. The service knows which happened and now says so.
        """
        assert dispatch(services, repo, "map", "--max-files", "0") == EXIT_OK
        out = capsys.readouterr().out

        assert "no ranked files" in out
        assert "--max-files kept none of the 2 ranked files" in out
        assert "nothing in this repository parsed into symbols" not in out

    def test_a_genuinely_empty_repository_still_says_so(self, services, tmp_path, capsys):
        # The other half of the same branch: when there really are no
        # candidates, the honest message is the one that was always there.
        (tmp_path / "notes.txt").write_text("no code here\n", encoding="utf-8")

        assert dispatch(services, tmp_path, "map") == EXIT_OK
        assert "nothing in this repository parsed into symbols" in capsys.readouterr().out

    def test_communities_at_zero_reports_the_count_it_found(self, services, repo, capsys):
        assert dispatch(services, repo, "communities", "--limit", "0") == EXIT_OK
        out = capsys.readouterr().out

        assert "no communities: nothing in this repository" not in out
        assert "communities not listed" in out

    def test_cycles_does_not_clear_a_repository_that_has_one(self, services, tmp_path, capsys):
        """Was a strict xfail; the marker came off when stage 4a landed.

        render_cycles branched on the truncated list rather than on
        CycleReport.total, which graph_service computes before truncation, so
        `cycles --limit 0` answered "no import cycles" and exited 0 for the
        repository below. The other two members of this family mislead; this
        one cleared a repository that does not pass.
        """
        for name, content in CYCLE.items():
            (tmp_path / name).write_text(content, encoding="utf-8")

        assert dispatch(services, tmp_path, "cycles", "--limit", "0") == EXIT_OK
        assert "no import cycles" not in capsys.readouterr().out
