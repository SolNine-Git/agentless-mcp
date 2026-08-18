"""Scaffold-level checks: the package imports and the entry points are wired."""

import pytest

import agentless_mcp
from agentless_mcp import bootstrap


def test_package_imports():
    assert agentless_mcp.__name__ == "agentless_mcp"


@pytest.mark.parametrize("entry_point", [bootstrap.cli_main, bootstrap.mcp_main])
def test_entry_points_are_declared_but_unimplemented(entry_point):
    with pytest.raises(NotImplementedError, match="not implemented yet"):
        entry_point()
