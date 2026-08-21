"""Scaffold-level checks: the package imports and the entry points are wired.

The Phase-0 version of this file asserted both entry points raised
``NotImplementedError``. Phase 1b implemented them, so the pin moves to what
is actually invariant: ``cli_main`` builds a real service graph and returns an
exit code, and ``mcp_main`` is loadable without importing fastmcp at module
scope (the optional-extra contract).
"""

from importlib import resources
from pathlib import Path

import agentless_mcp
from agentless_mcp import bootstrap


def test_package_imports():
    assert agentless_mcp.__name__ == "agentless_mcp"


def test_cli_main_wires_a_real_service_graph(tmp_path, capsys):
    """The composition root builds working services, not placeholders."""
    (tmp_path / "sample.py").write_text("def f():\n    return 1\n", encoding="utf-8")

    code = bootstrap.cli_main(["tree", "--repo", str(tmp_path)])

    assert code == 0
    assert "sample.py" in capsys.readouterr().out


def test_mcp_entry_point_does_not_import_fastmcp_at_module_scope():
    """A CLI-only install must not pay for -- or trip over -- the mcp extra.

    ``bootstrap`` is imported by the CLI console script, so the server module
    it loads has to stay out of its import graph until ``mcp_main`` runs.
    """
    source = Path(bootstrap.__file__).read_text(encoding="utf-8")
    assert "import fastmcp" not in source
    assert "from fastmcp" not in source
    assert bootstrap.SERVER_MODULE == "agentless_mcp.adapters.mcp.server"


def test_the_agent_guide_ships_as_package_data():
    """An install-only user has no checkout, so the guide must be in the wheel.

    Anchored on ``agentless_mcp`` rather than ``agentless_mcp.docs``: the docs
    directory holds data and carries no ``__init__``, and ``files()`` only
    handles a namespace package reliably from 3.12.
    """
    resource = resources.files("agentless_mcp") / "docs" / "agent-guide.md"

    assert resource.is_file()
    assert resource.read_text(encoding="utf-8").startswith("# agentless-mcp: agent usage guide")
