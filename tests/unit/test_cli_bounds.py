"""Every numeric CLI option at 0, at -1, and above its cap.

`agentless_mcp.util.bounds` owns the rule and the numbers; every service
calls it, and the MCP adapter re-exports the same constants into the JSON
schema it publishes. That is what makes this table meaningful across both
front doors rather than only this one.

The exit-code table is deliberately exhaustive rather than representative:
the value of a characterization test is that the diff after a change shows
every cell that moved, and a sampled table hides the cells nobody thought to
sample.

Three of these are worse than inconsistent. `cycles`, `map` and `communities`
answer a bounded question by reporting that the repository is empty, so a
caller who passes zero is told a fact about the repository rather than about
the bound. Those get their own tests below the table.
"""

from __future__ import annotations

import re

import pytest

from agentless_mcp.adapters.cli.formatting import EXIT_DOMAIN, EXIT_OK
from agentless_mcp.adapters.cli.main import CliServices, run
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.lint_service import LintService
from agentless_mcp.application.map_service import MapResult, MapService
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.validate_service import ValidateRequest, ValidateService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.util.errors import AgentlessError

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
# means a service refused it.
#
# The "above cap" column used to read EXIT_OK on fourteen of these rows, and
# that expectation was wrong rather than merely lenient: the MCP adapter has
# always refused those same values through `Field(le=)`, so the table was
# pinning the divergence between the two front doors as if it were the
# contract. `cycles --limit 100000` was answered on the command line and
# refused over MCP for the same repository. The services now hold the
# published ceiling, so both doors refuse it and this column says so.
#
# Two rows still read EXIT_OK above the cap, and they are not oversights.
# `--members` and `--max-visited` are reachable from the command line only;
# with no second door publishing a ceiling there is no number to agree with,
# and neither value misleads -- a huge `--members` lists every member it has,
# a huge `--max-visited` simply never trips.
BOUNDARY_EXITS: dict[tuple[str, str], tuple[int, int, int]] = {
    #                                        0            -1        above cap
    ("communities", "--resolution"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    ("communities", "--limit"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    # CLI-only: no published ceiling to match. See the note above.
    ("communities", "--members"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_OK),
    ("cycles", "--limit"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    # The floor is zero for the reason the html twin below gives.
    ("diagram", "--max-edges"): (EXIT_OK, EXIT_DOMAIN, EXIT_DOMAIN),
    ("diagram", "--max-nodes"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    ("diagram", "--resolution"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    ("expand", "--limit"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    ("explain", "--limit"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    ("find-symbol", "--limit"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    ("html", "--max-nodes"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    # The one option whose floor is zero rather than one: no reference edges
    # is a legible diagram, where no nodes is not a diagram.
    ("html", "--max-edges"): (EXIT_OK, EXIT_DOMAIN, EXIT_DOMAIN),
    ("html", "--resolution"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    ("map", "--max-files"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    # CLI-only: no published ceiling to match. See the note above.
    ("path", "--max-visited"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_OK),
    ("refs", "--limit"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    # Context lines are a span around a match, so zero of them is an answer.
    ("resolve-locs", "--context"): (EXIT_OK, EXIT_DOMAIN, EXIT_DOMAIN),
    ("slice", "--context"): (EXIT_OK, EXIT_DOMAIN, EXIT_DOMAIN),
    ("tree", "--depth"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
    ("tree", "--max-entries"): (EXIT_DOMAIN, EXIT_DOMAIN, EXIT_DOMAIN),
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

    def test_one_refusal_vocabulary_is_in_use(self, services, repo, capsys):
        """Three rules in three layers became one rule in one.

        Before stage 4b, `html --max-nodes 0` was refused by a range check in
        the CLI, `communities --limit -1` by a sign check in the CLI, and
        `refs --limit 0` by `symbol_service._check_limit` -- three wordings,
        two exit codes, and no shared rule. The services now own it and both
        front doors inherit it, so the wording is one sentence with the
        parameter's own name in it.
        """
        messages = []
        for command, option, value in (
            ("html", "--max-nodes", "0"),
            ("communities", "--limit", "-1"),
            ("refs", "--limit", "0"),
            ("tree", "--depth", "0"),
            ("communities", "--members", "0"),
        ):
            dispatch(services, repo, command, option, value)
            # The last line, because the fixture is not a git checkout and the
            # degradation warning now precedes the refusal on stderr for a
            # --repo invocation as well as for a cwd one.
            messages.append(capsys.readouterr().err.strip().splitlines()[-1])

        # Every one of these now names a range rather than a floor, because
        # every one of them now has a ceiling the MCP door already published.
        # `--members` is the parameter that still reads "at least", and it is
        # the one with no second door to agree with.
        assert messages == [
            "agentless-mcp: max_nodes takes a value from 1 through 1000, got 0",
            "agentless-mcp: limit takes a value from 1 through 500, got -1",
            "agentless-mcp: limit takes a value from 1 through 500, got 0",
            "agentless-mcp: depth takes a value from 1 through 20, got 0",
            "agentless-mcp: members must be at least 1, got 0",
        ]


class TestABoundOfZeroIsRefused:
    """A bound that cannot bound is a caller mistake, not a question.

    All three of these used to answer it: `map --max-files 0` reported the
    repository as having nothing to rank, `communities --limit 0` reported no
    communities, and `cycles --limit 0` reported no import cycles for a
    repository that has one. The first two misled; the third cleared
    something.
    """

    @pytest.mark.parametrize(
        ("command", "option"),
        [("communities", "--limit"), ("cycles", "--limit")],
    )
    def test_the_answer_is_a_refusal_naming_the_parameter(
        self, services, repo, capsys, command, option
    ):
        assert dispatch(services, repo, command, option, "0") == EXIT_DOMAIN
        assert "limit takes a value from 1 through 500, got 0" in capsys.readouterr().err

    def test_a_map_file_limit_is_refused_against_its_published_range(self, services, repo, capsys):
        # max_files has a ceiling as well as a floor, so it reports the range
        # rather than the floor alone.
        assert dispatch(services, repo, "map", "--max-files", "0") == EXIT_DOMAIN
        assert "max_files takes a value from 1 through 200, got 0" in capsys.readouterr().err

    def test_cycles_at_its_smallest_real_limit_still_finds_the_cycle(
        self, services, tmp_path, capsys
    ):
        # The other half of 4a: with a limit it can honour, the count comes
        # from `report.total` -- computed before truncation -- so a truncated
        # listing still says how many there are.
        for name, content in CYCLE.items():
            (tmp_path / name).write_text(content, encoding="utf-8")

        assert dispatch(services, tmp_path, "cycles", "--limit", "1") == EXIT_OK
        assert "1 import cycle" in capsys.readouterr().out


class TestAnEmptyMapNamesItsCause:
    """`render_map` reports rows; the service reports why there are none.

    Driven through `MapService.render_text` rather than the CLI because two
    of the three causes are no longer reachable from a command line -- a
    max_files of zero is refused now -- and the branch still has to be right
    for a library caller who builds the result themselves.
    """

    def render(self, extractor, counter, **fields):
        result = MapResult(
            files=(), budget=0, included=0, candidates=0, seeds=(), skipped=(), **fields
        )
        return MapService(extractor, counter).render_text(result)

    def test_nothing_ranked_at_all_is_a_fact_about_the_repository(self, extractor, counter):
        assert "nothing in this repository parsed into symbols" in self.render(extractor, counter)

    def test_a_file_limit_that_kept_none_says_so(self, extractor, counter):
        assert "--max-files kept none of the 7 ranked files" in self.render(
            extractor, counter, ranked=7
        )

    def test_a_budget_that_fitted_none_names_the_budget(self, extractor, counter):
        result = MapResult(
            files=(), budget=250, included=0, candidates=31, seeds=(), skipped=(), ranked=7
        )
        text = MapService(extractor, counter).render_text(result)

        assert "the 250-token budget left room for none of 31 symbols" in text
        assert "nothing in this repository parsed into symbols" not in text


class TestValidateBoundsAreRefused:
    """The four numbers a validation run takes, none of which was checked.

    Not reached by the exit-code table above, because running them means
    running a test suite. Driven through the service, which is where the
    rule now lives.
    """

    @pytest.fixture
    def service(self, extractor):
        return ValidateService(PatchService(extractor))

    @pytest.mark.parametrize(
        "case",
        [
            ("jobs", 0, "jobs takes a value from 1 through 200, got 0"),
            ("jobs", -1, "jobs takes a value from 1 through 200, got -1"),
            ("timeout", 0, "timeout must be at least 1, got 0"),
            ("repeat_baseline", 0, "repeat_baseline must be at least 1, got 0"),
        ],
        ids=["jobs-0", "jobs-negative", "timeout-0", "repeat-0"],
    )
    def test_a_bound_that_cannot_bound_is_refused(self, service, tmp_path, case):
        field, value, expected = case
        candidates = tmp_path / "candidates"
        candidates.mkdir()
        request = ValidateRequest(candidates=candidates, test_cmd="true", **{field: value})

        with pytest.raises(AgentlessError, match=re.escape(expected)):
            service.validate(resolve_repo(tmp_path, None), request)

    def test_a_zero_repeat_used_to_be_silently_rewritten_to_one(self, service, tmp_path):
        # `repeats = max(1, request.repeat_baseline)` ran the baseline once
        # and reported it as one run, so a caller who asked for none could
        # not tell their request had been rewritten.
        candidates = tmp_path / "candidates"
        candidates.mkdir()
        request = ValidateRequest(candidates=candidates, test_cmd="true", repeat_baseline=0)

        with pytest.raises(AgentlessError):
            service.validate(resolve_repo(tmp_path, None), request)
