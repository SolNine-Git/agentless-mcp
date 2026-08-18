"""Scaffold-level checks: the package imports and the entry points are wired.

The Phase-0 version of this file asserted both entry points raised
``NotImplementedError``. Phase 1b implemented them, so the pin moves to what
is actually invariant: ``cli_main`` builds a real service graph and returns an
exit code, and ``mcp_main`` is loadable without importing fastmcp at module
scope (the optional-extra contract).
"""

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
