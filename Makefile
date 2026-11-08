# Writes the agentless-mcp hook entries into the user's Claude Code settings. The binary is the
# installed build, never the build under test.
AGENTLESS_MCP ?= $(shell uv tool dir --bin)/agentless-mcp

.PHONY: install-hooks
install-hooks:
	$(AGENTLESS_MCP) hook install
