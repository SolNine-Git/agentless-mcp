"""Module entry point: ``python -m agentless_mcp`` runs the CLI bootstrap."""

from agentless_mcp.bootstrap import cli_main

if __name__ == "__main__":
    raise SystemExit(cli_main())
