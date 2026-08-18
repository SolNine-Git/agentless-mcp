"""The MCP adapter: allowlist behaviour, annotations, and a real round trip.

FastMCP ships an in-memory transport (``Client(server)``), so these are true
end-to-end calls through tool registration, schema validation and dispatch --
not handler functions called directly. The handler layer is exercised too,
because the refusal cases are easier to assert without a JSON-RPC error
wrapper around them.
"""

import asyncio

import pytest
from fastmcp import Client

from agentless_mcp.adapters.mcp.annotations import read_only
from agentless_mcp.adapters.mcp.server import (
    ServerServices,
    ToolHandlers,
    build_server,
    parse_args,
)
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.util.errors import SecurityRefusal

EXPECTED_TOOLS = {
    "repo_map",
    "list_dir",
    "get_symbols_overview",
    "expand_symbols",
    "read_slice",
    "find_symbol",
    "find_referencing_symbols",
    "resolve_locations",
    "capabilities",
}

SOURCE = """\
def quote(sku):
    return 1


class PriceBook:
    def cost_of(self, sku):
        return quote(sku)
"""


@pytest.fixture
def services(extractor, counter):
    return ServerServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor),
        counter=counter,
    )


@pytest.fixture
def one_repo(tmp_path):
    root = tmp_path / "alpha"
    root.mkdir()
    (root / "core.py").write_text(SOURCE, encoding="utf-8")
    return root


@pytest.fixture
def two_repos(tmp_path, one_repo):
    other = tmp_path / "beta"
    other.mkdir()
    (other / "other.py").write_text(SOURCE, encoding="utf-8")
    return [one_repo, other]


class TestAllowlist:
    def test_one_root_is_the_default_for_an_omitted_repo_root(self, services, one_repo):
        handlers = ToolHandlers([one_repo], services)
        assert handlers.resolve(None).root == one_repo.resolve()

    def test_several_roots_refuse_to_guess_and_list_themselves(self, services, two_repos):
        handlers = ToolHandlers(two_repos, services)
        with pytest.raises(SecurityRefusal) as caught:
            handlers.resolve(None)

        message = str(caught.value)
        assert "will not guess" in message
        for root in two_repos:
            assert str(root.resolve()) in message

    def test_a_blank_repo_root_is_treated_as_omitted(self, services, two_repos):
        handlers = ToolHandlers(two_repos, services)
        with pytest.raises(SecurityRefusal):
            handlers.resolve("   ")

    def test_an_unlisted_root_is_refused(self, services, two_repos, tmp_path):
        handlers = ToolHandlers(two_repos, services)
        outside = tmp_path / "gamma"
        outside.mkdir()
        with pytest.raises(SecurityRefusal, match="not one of this server's roots"):
            handlers.resolve(str(outside))

    def test_no_roots_at_all_means_no_service(self, services):
        with pytest.raises(SecurityRefusal, match="no repositories are served"):
            ToolHandlers([], services).resolve(None)

    def test_client_roots_are_additive_to_the_configured_ones(self, services, two_repos):
        handlers = ToolHandlers([two_repos[0]], services)
        resolved = handlers.resolve(str(two_repos[1]), [two_repos[1]])
        assert resolved.root == two_repos[1].resolve()

    def test_a_single_client_root_can_be_the_default(self, services, one_repo):
        handlers = ToolHandlers([], services)
        assert handlers.resolve(None, [one_repo]).root == one_repo.resolve()


def listed_tools(server):
    """List the tools as a client sees them, annotations included."""

    async def go():
        async with Client(server) as client:
            return await client.list_tools()

    return asyncio.run(go())


class TestAnnotations:
    def test_every_tool_is_annotated_read_only(self, services, one_repo):
        tools = listed_tools(build_server(ToolHandlers([one_repo], services)))

        assert {tool.name for tool in tools} == EXPECTED_TOOLS
        for tool in tools:
            annotations = tool.annotations
            assert annotations is not None, tool.name
            assert annotations.readOnlyHint is True, tool.name
            assert annotations.destructiveHint is False, tool.name
            assert annotations.openWorldHint is False, tool.name
            assert annotations.idempotentHint is True, tool.name

    def test_the_annotation_helper_carries_the_documented_hints(self):
        assert read_only("X") == {
            "title": "X",
            "readOnlyHint": True,
            "destructiveHint": False,
            "idempotentHint": True,
            "openWorldHint": False,
        }

    def test_there_is_no_write_exec_or_fetch_tool(self, services, one_repo):
        names = {
            tool.name for tool in listed_tools(build_server(ToolHandlers([one_repo], services)))
        }
        forbidden = {"write", "exec", "shell", "fetch", "apply", "patch", "run"}
        assert not any(word in name for name in names for word in forbidden)


class TestRoundTrip:
    """Real calls over FastMCP's in-memory transport."""

    def call(self, server, tool, arguments):
        async def go():
            async with Client(server) as client:
                return await client.call_tool(tool, arguments)

        return asyncio.run(go())

    def test_repo_map_answers_with_a_receipt(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services))
        result = self.call(server, "repo_map", {"repo_root": str(one_repo)})
        text = result.content[0].text

        assert text.startswith("# agentless-mcp receipt\n")
        assert "py:core.py::quote" in text

    def test_an_omitted_repo_root_defaults_when_there_is_one_root(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services))
        text = self.call(server, "repo_map", {}).content[0].text
        assert str(one_repo.resolve()) in text

    def test_an_omitted_repo_root_is_refused_when_there_are_several(self, services, two_repos):
        server = build_server(ToolHandlers(two_repos, services))
        with pytest.raises(Exception, match="will not guess"):
            self.call(server, "repo_map", {})

    def test_find_symbol_returns_incident_cards(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services))
        text = (
            self.call(server, "find_symbol", {"name": "cost_of", "repo_root": str(one_repo)})
            .content[0]
            .text
        )

        assert "py:core.py::PriceBook.cost_of" in text
        assert "core.py:6-7" in text

    def test_expand_symbols_returns_a_numbered_body(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services))
        text = (
            self.call(
                server,
                "expand_symbols",
                {"stable_ids": ["py:core.py::quote"], "repo_root": str(one_repo)},
            )
            .content[0]
            .text
        )

        assert "1| def quote(sku):" in text

    def test_capabilities_names_the_roots_it_serves(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services))
        text = self.call(server, "capabilities", {"repo_root": str(one_repo)}).content[0].text
        assert f"roots: {one_repo.resolve()}" in text


class TestServerArguments:
    def test_root_is_repeatable(self):
        assert parse_args(["--root", "/a", "--root", "/b"]).root == ["/a", "/b"]

    def test_no_roots_parses_to_an_empty_list(self):
        assert parse_args([]).root == []
