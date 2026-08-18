"""Composition root: builds the object graph and hands it to an adapter.

Deliberately outside the import-linter layer contract -- it wires concrete
implementations into the adapters layer, so by construction it sits above
every layer and cannot be a member of one.
"""


def cli_main() -> int:
    """Entry point for the ``agentless-mcp`` console script."""
    message = "agentless-mcp CLI is not implemented yet (Phase 0 scaffold)."
    raise NotImplementedError(message)


def mcp_main() -> int:
    """Entry point for the ``agentless-mcp-server`` stdio MCP script."""
    message = "agentless-mcp MCP server is not implemented yet (Phase 0 scaffold)."
    raise NotImplementedError(message)
