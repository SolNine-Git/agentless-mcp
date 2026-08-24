"""The two front doors are independent by contract and must still agree.

``pyproject.toml`` enforces that ``adapters.cli`` and ``adapters.mcp`` never
import each other, and that is right: either importing the other would make the
CLI drag in fastmcp or the server inherit argparse's exit-code semantics. But
independence of *implementation* was allowed to become divergence of
*contract*, because nothing held the two surfaces together.

The concrete gap this file closes: every numeric parameter the MCP adapter
publishes carries a ``Field(ge=, le=)`` bound, and every numeric CLI argument
for the same parameter is a bare ``type=int``. An out-of-range value is refused
by one front door and silently honoured by the other, which is how
``cycles --limit 0`` came to report "no import cycles" for a repository that has
one.

This module is the inventory gate. It pins which CLI options take a number, so
adding another one is a visible diff rather than a silent widening of the
unbounded surface, and it records which MCP bounds exist to be matched. The
bound *enforcement* lives with the services (see
``application/symbol_service._check_limit``); these tests assert the surface,
not the refusal.
"""

from __future__ import annotations

import argparse

import pytest

from agentless_mcp.adapters.cli import main as cli
from agentless_mcp.core import projectconfig

# Every (command, option) on the CLI that accepts a number, as of the audit
# remediation. A new entry here is a new parameter that a service must bound;
# a disappearing entry means a command lost an option. Either is worth seeing
# in a diff, which is the whole point of pinning the set.
NUMERIC_CLI_OPTIONS: frozenset[tuple[str, str]] = frozenset(
    {
        ("communities", "--limit"),
        ("communities", "--members"),
        ("communities", "--resolution"),
        ("cycles", "--limit"),
        ("diagram", "--max-edges"),
        ("diagram", "--max-nodes"),
        ("diagram", "--resolution"),
        ("expand", "--limit"),
        ("explain", "--limit"),
        ("find-symbol", "--limit"),
        ("html", "--max-edges"),
        ("html", "--max-nodes"),
        ("html", "--resolution"),
        ("map", "--max-files"),
        ("path", "--max-visited"),
        ("refs", "--limit"),
        ("resolve-locs", "--context"),
        ("slice", "--context"),
        ("tree", "--depth"),
        ("tree", "--max-entries"),
        ("validate", "--jobs"),
        ("validate", "--repeat-baseline"),
        ("validate", "--run-timeout"),
        ("validate", "--timeout"),
    }
)


def _numeric_cli_options() -> set[tuple[str, str]]:
    """Return every (command, option) pair on the CLI that parses a number."""
    # argparse exposes no public walk over a built parser, so the private
    # action list is the only way to ask "what did this surface declare".
    parser = cli.build_parser()
    subparsers = [
        action for action in parser._actions if isinstance(action, argparse._SubParsersAction)
    ]
    found: set[tuple[str, str]] = set()
    for group in subparsers:
        for command, subparser in group.choices.items():
            for action in subparser._actions:
                if action.type in (int, float) and action.option_strings:
                    found.add((command, action.option_strings[0]))
    return found


class TestNumericSurface:
    def test_the_set_of_numeric_cli_options_is_pinned(self):
        assert _numeric_cli_options() == set(NUMERIC_CLI_OPTIONS)

    def test_every_pinned_option_still_exists(self):
        # Guards the inverse of the test above with a clearer failure: a
        # removed command reads as a missing pair rather than a set diff.
        live = _numeric_cli_options()
        missing = sorted(pair for pair in NUMERIC_CLI_OPTIONS if pair not in live)
        assert not missing, f"pinned CLI options no longer exist: {missing}"


class TestTheOneNumericOptionTheTypeWalkCannotSee:
    """``map --budget`` takes a number or the word 'auto', so it is type=str.

    That keeps it out of the inventory above, which is why it is pinned here
    instead. The MCP wire publishes ``Field(ge=MIN_BUDGET, le=MAX_BUDGET)`` for
    the same parameter and ``projectconfig`` enforces the same range for a
    ``.agentless-mcp.json`` entry, so the command line is the one of the three
    doors with no bound behind it.
    """

    def test_budget_is_declared_as_text_not_a_number(self):
        parser = cli.build_parser()
        subparsers = [
            action for action in parser._actions if isinstance(action, argparse._SubParsersAction)
        ]
        budget = next(
            action
            for group in subparsers
            for action in group.choices["map"]._actions
            if "--budget" in action.option_strings
        )
        assert budget.type is None
        assert ("map", "--budget") not in NUMERIC_CLI_OPTIONS

    def test_the_published_range_is_the_one_the_config_file_enforces(self):
        assert projectconfig.MIN_BUDGET == 200
        assert projectconfig.MAX_BUDGET == 64_000


class TestPublishedBounds:
    """The MCP adapter's bounds are the numbers the CLI has to match."""

    def test_the_wire_bounds_are_the_published_ones(self):
        server = pytest.importorskip(
            "agentless_mcp.adapters.mcp.server",
            reason="the mcp extra is not installed",
        )
        # These four constants are what the published JSON schema advertises.
        # A change here is a change to what an agent is told before it calls,
        # so it should never be incidental.
        assert server.MAX_LIMIT == 500
        assert server.MAX_CONTEXT_LINES == 200
        assert server.MAX_DIAGRAM_NODES == 500
        assert server.MAX_RESOLUTION == 100.0
