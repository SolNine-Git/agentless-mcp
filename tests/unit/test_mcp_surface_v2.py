"""The v2 consolidated surface: parity with v1, and the runtime rejection story.

Two guarantees pinned here. *Parity*: every v2 operation is adapter-layer
routing into the same handler its v1 counterpart tool calls, so its answer --
receipt, banner and content -- is byte-identical to the v1 tool's on the same
repository. *Rejection*: the v2 tools publish ``operation`` as a plain string,
no wire enum, so the server's own message is what reaches the agent, and for
every rejection class -- unknown operation, a parameter foreign to the
selected operation, a missing required parameter -- it names the operation,
what it accepts and what it requires, never a schema validation dump.
"""

import asyncio
import json

import pytest
from fastmcp import Client
from fastmcp.exceptions import ToolError

from agentless_mcp.adapters.mcp import server as server_module
from agentless_mcp.adapters.mcp.server import (
    SURFACE_BOTH,
    SURFACE_V1,
    SURFACE_V2,
    SURFACES,
    ServerServices,
    ToolHandlers,
    build_server,
    parse_args,
)
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.view_service import ViewService

EXPECTED_TOOLS_V2 = {
    "orient",
    "symbols",
    "find_referencing_symbols",
    "read",
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
        symbols=SymbolService(extractor, counter),
        graphs=GraphService(extractor),
        counter=counter,
        extractor=extractor,
    )


@pytest.fixture
def one_repo(tmp_path):
    root = tmp_path / "alpha"
    root.mkdir()
    (root / "core.py").write_text(SOURCE, encoding="utf-8")
    (root / "src").mkdir()
    (root / "src" / "inside.py").write_text("VALUE = 1\n", encoding="utf-8")
    return root


def listed_tools(server):
    """List the tools as a client sees them, annotations included."""

    async def go():
        async with Client(server) as client:
            return await client.list_tools()

    return asyncio.run(go())


def call(server, tool, arguments):
    """Call one tool through a real client session."""

    async def go():
        async with Client(server) as client:
            return await client.call_tool(tool, arguments)

    return asyncio.run(go())


# Every v2 operation beside its v1 counterpart, defaults and every optional
# parameter exercised. The parity test requires each pair to answer with the
# same bytes over the same fixture repository.
PARITY_PAIRS = [
    ("orient", {"operation": "map"}, "repo_map", {}),
    (
        "orient",
        {"operation": "map", "focus": "quote", "budget": 2000, "limit": 1},
        "repo_map",
        {"focus": "quote", "budget": 2000, "max_files": 1},
    ),
    (
        "orient",
        {"operation": "map", "granularity": "file"},
        "repo_map",
        {"granularity": "file"},
    ),
    (
        "orient",
        {"operation": "communities"},
        "analyze_structure",
        {"operation": "communities"},
    ),
    (
        "orient",
        {"operation": "communities", "resolution": 2.0, "limit": 3},
        "analyze_structure",
        {"operation": "communities", "resolution": 2.0, "limit": 3},
    ),
    ("orient", {"operation": "cycles"}, "analyze_structure", {"operation": "cycles"}),
    (
        "orient",
        {"operation": "cycles", "limit": 2},
        "analyze_structure",
        {"operation": "cycles", "limit": 2},
    ),
    ("orient", {"operation": "diagram"}, "analyze_structure", {"operation": "diagram"}),
    (
        "orient",
        {
            "operation": "diagram",
            "focus": "core.py",
            "max_nodes": 5,
            "group_by_communities": True,
        },
        "analyze_structure",
        {
            "operation": "diagram",
            "focus": "core.py",
            "max_nodes": 5,
            "group_by_communities": True,
        },
    ),
    (
        "orient",
        {"operation": "path", "source": "PriceBook.cost_of", "target": "quote"},
        "analyze_structure",
        {"operation": "path", "source": "PriceBook.cost_of", "target": "quote"},
    ),
    (
        "orient",
        {
            "operation": "path",
            "source": "PriceBook.cost_of",
            "target": "quote",
            "include_unique": True,
            "include_ambiguous": True,
        },
        "analyze_structure",
        {
            "operation": "path",
            "source": "PriceBook.cost_of",
            "target": "quote",
            "include_unique": True,
            "include_ambiguous": True,
        },
    ),
    ("symbols", {"operation": "find", "name": "quote"}, "find_symbol", {"name": "quote"}),
    (
        "symbols",
        {"operation": "find", "name": "quote", "kind": "function", "limit": 5},
        "find_symbol",
        {"name": "quote", "kind": "function", "limit": 5},
    ),
    (
        "symbols",
        {"operation": "overview", "paths": ["core.py"]},
        "get_symbols_overview",
        {"paths": ["core.py"]},
    ),
    (
        "symbols",
        {"operation": "overview", "paths": ["core.py"], "docstrings": True},
        "get_symbols_overview",
        {"paths": ["core.py"], "docstrings": True},
    ),
    (
        "symbols",
        {"operation": "expand", "stable_ids": ["py:core.py::quote"]},
        "expand_symbols",
        {"stable_ids": ["py:core.py::quote"]},
    ),
    (
        "symbols",
        {"operation": "expand", "stable_ids": ["py:core.py::quote"], "limit": 5},
        "expand_symbols",
        {"stable_ids": ["py:core.py::quote"], "limit": 5},
    ),
    ("symbols", {"operation": "explain", "target": "quote"}, "explain_symbol", {"target": "quote"}),
    (
        "symbols",
        {"operation": "explain", "target": "quote", "limit": 5},
        "explain_symbol",
        {"target": "quote", "limit": 5},
    ),
    (
        "symbols",
        {"operation": "locate", "path": "core.py", "locations": ["function:quote"]},
        "resolve_locations",
        {"path": "core.py", "locs": ["function:quote"]},
    ),
    (
        "symbols",
        {
            "operation": "locate",
            "path": "core.py",
            "locations": ["class:PriceBook"],
            "context_lines": 1,
        },
        "resolve_locations",
        {"path": "core.py", "locs": ["class:PriceBook"], "context_lines": 1},
    ),
    (
        "read",
        {"operation": "slice", "path": "core.py", "lines": [[1, 2]]},
        "read_slice",
        {"path": "core.py", "lines": [[1, 2]]},
    ),
    (
        "read",
        {"operation": "slice", "path": "core.py", "lines": [[1, 2]], "context_lines": 0},
        "read_slice",
        {"path": "core.py", "lines": [[1, 2]], "context_lines": 0},
    ),
    (
        "read",
        {"operation": "slice", "path": "core.py", "whole_file": True},
        "read_slice",
        {"path": "core.py", "whole_file": True},
    ),
    ("read", {"operation": "dir"}, "list_dir", {}),
    (
        "read",
        {"operation": "dir", "path": "src", "depth": 1, "max_entries": 5},
        "list_dir",
        {"path": "src", "depth": 1, "max_entries": 5},
    ),
]


class TestParity:
    """Every v2 operation answers byte-identically to its v1 counterpart tool."""

    @pytest.mark.parametrize(
        "pair",
        PARITY_PAIRS,
        ids=[f"{tool}-{arguments['operation']}" for tool, arguments, _, _ in PARITY_PAIRS],
    )
    def test_a_v2_operation_matches_its_v1_counterpart(self, services, one_repo, pair):
        v2_tool, v2_arguments, v1_tool, v1_arguments = pair
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_BOTH)

        async def go():
            async with Client(server) as client:
                consolidated = await client.call_tool(
                    v2_tool, {"repo_root": str(one_repo), **v2_arguments}
                )
                original = await client.call_tool(
                    v1_tool, {"repo_root": str(one_repo), **v1_arguments}
                )
                return consolidated, original

        consolidated, original = asyncio.run(go())
        assert consolidated.content[0].text == original.content[0].text

    def test_every_orient_and_symbols_and_read_operation_is_covered(self):
        """The parity table spans the full operation vocabulary of each tool."""
        covered = {(tool, arguments["operation"]) for tool, arguments, _, _ in PARITY_PAIRS}
        expected = (
            {("orient", name) for name in server_module.ORIENT_OPERATIONS}
            | {("symbols", name) for name in server_module.SYMBOLS_OPERATIONS}
            | {("read", name) for name in server_module.READ_OPERATIONS}
        )
        assert covered == expected


class TestSurfaceListing:
    def test_v2_publishes_exactly_five_tools(self, services, one_repo):
        tools = listed_tools(build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2))
        assert {tool.name for tool in tools} == EXPECTED_TOOLS_V2

    def test_v2_is_the_default_surface(self, services, one_repo):
        tools = listed_tools(build_server(ToolHandlers([one_repo], services)))
        assert {tool.name for tool in tools} == EXPECTED_TOOLS_V2

    def test_both_publishes_the_union_of_the_surfaces(self, services, one_repo):
        # find_referencing_symbols and capabilities are shared, so the union
        # is fourteen names rather than sixteen.
        tools = listed_tools(build_server(ToolHandlers([one_repo], services), surface=SURFACE_BOTH))
        names = {tool.name for tool in tools}
        assert names >= EXPECTED_TOOLS_V2
        assert "repo_map" in names
        assert "analyze_structure" in names
        assert len(tools) == 14

    def test_every_v2_tool_is_annotated_read_only(self, services, one_repo):
        tools = listed_tools(build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2))
        for tool in tools:
            annotations = tool.annotations
            assert annotations is not None, tool.name
            assert annotations.readOnlyHint is True, tool.name
            assert annotations.destructiveHint is False, tool.name

    def test_every_v2_parameter_has_a_schema_description(self, services, one_repo):
        tools = listed_tools(build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2))
        for tool in tools:
            for name, schema in tool.inputSchema.get("properties", {}).items():
                rendered = json.dumps(schema)
                assert '"description"' in rendered, f"{tool.name}.{name}"


class TestOperationSchema:
    """The v2 rejection story: no wire enum, the server's own message instead.

    The spike behind issue #17 measured both shapes: a discriminated union
    only publishes through a nested wrapper property, which breaks flat calls
    (the #13 bridge class), and a wire enum answers a wrong value with a
    pydantic literal_error that names the values but not the tool or its
    per-operation parameters. The flat schema with runtime validation is the
    one shape whose refusals can name the whole fix, so operation publishes
    as a plain required string and this class pins that.
    """

    def test_operation_publishes_as_a_plain_required_string(self, services, one_repo):
        listed = listed_tools(build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2))
        tools = {tool.name: tool for tool in listed}
        for name in ("orient", "symbols", "read"):
            schema = tools[name].inputSchema
            operation = schema["properties"]["operation"]
            assert operation.get("type") == "string", name
            assert "enum" not in operation, name
            assert "const" not in operation, name
            assert "operation" in schema.get("required", []), name


REJECTIONS = [
    # Unknown operation: answered with the tool and the full valid list.
    (
        "orient",
        {"operation": "graph"},
        ["orient has no operation named 'graph'", "communities, cycles, diagram, map, path"],
    ),
    (
        "symbols",
        {"operation": "grep"},
        ["symbols has no operation named 'grep'", "expand, explain, find, locate, overview"],
    ),
    (
        "read",
        {"operation": "tree"},
        ["read has no operation named 'tree'", "dir, slice"],
    ),
    # A parameter foreign to the selected operation: the refusal names the
    # operation, its accepted parameters and its required ones.
    (
        "orient",
        {"operation": "map", "source": "quote"},
        [
            "orient operation 'map' does not accept: source",
            "accepts: focus, budget, limit, granularity",
            "required: none",
        ],
    ),
    (
        "symbols",
        {"operation": "find", "name": "quote", "stable_ids": ["py:core.py::quote"]},
        [
            "symbols operation 'find' does not accept: stable_ids",
            "accepts: name, kind, limit",
            "required: name",
        ],
    ),
    (
        "read",
        {"operation": "dir", "lines": [[1, 2]]},
        [
            "read operation 'dir' does not accept: lines",
            "accepts: path, depth, max_entries",
            "required: none",
        ],
    ),
    # A missing required-for-operation parameter, blank strings included.
    (
        "orient",
        {"operation": "path", "source": "quote"},
        [
            "orient operation 'path' is missing: target",
            "accepts: source, target, include_unique, include_ambiguous",
            "required: source, target",
        ],
    ),
    (
        "orient",
        {"operation": "path", "source": "quote", "target": "  "},
        ["orient operation 'path' is missing: target"],
    ),
    (
        "symbols",
        {"operation": "find"},
        ["symbols operation 'find' is missing: name", "required: name"],
    ),
    (
        "symbols",
        {"operation": "locate", "path": "core.py"},
        ["symbols operation 'locate' is missing: locations", "required: path, locations"],
    ),
    (
        "read",
        {"operation": "slice"},
        ["read operation 'slice' is missing: path", "required: path"],
    ),
]


class TestRejections:
    """Each rejection class answers with the fix, never a pydantic dump."""

    @pytest.mark.parametrize(
        ("tool", "arguments", "expected"),
        REJECTIONS,
        ids=[f"{tool}-{arguments['operation']}" for tool, arguments, _ in REJECTIONS],
    )
    def test_the_refusal_names_the_fix(self, services, one_repo, tool, arguments, expected):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        with pytest.raises(ToolError) as raised:
            call(server, tool, {"repo_root": str(one_repo), **arguments})

        message = str(raised.value)
        for fragment in expected:
            assert fragment in message, message
        assert "validation error" not in message, message

    def test_a_slice_refusal_blames_the_read_tool_not_read_slice(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        with pytest.raises(ToolError, match="read operation 'slice' requires non-empty lines"):
            call(
                server,
                "read",
                {"repo_root": str(one_repo), "operation": "slice", "path": "core.py"},
            )


class TestSurfaceFlag:
    def test_v2_is_the_default_and_the_choices_parse(self):
        assert parse_args([]).surface == SURFACE_V2
        for surface in SURFACES:
            assert parse_args(["--surface", surface]).surface == surface

    def test_an_unknown_surface_is_refused(self, capsys):
        with pytest.raises(SystemExit) as caught:
            parse_args(["--surface", "v3"])
        assert caught.value.code == 2
        assert "received argv" in capsys.readouterr().err

    def test_serve_builds_the_surface_the_flag_selected(self, services, tmp_path, monkeypatch):
        built = {}

        class StubTransport:
            def run(self, **kwargs):
                _ = kwargs

        def record(handlers, surface):
            built["surface"] = surface
            return StubTransport()

        monkeypatch.setattr(server_module, "build_server", record)
        monkeypatch.setattr(server_module.grammars, "start_auto_warm", lambda *a, **k: None)

        assert (
            server_module.serve(["--root", str(tmp_path), "--surface", SURFACE_V1], services) == 0
        )
        assert built["surface"] == SURFACE_V1

        assert server_module.serve(["--root", str(tmp_path)], services) == 0
        assert built["surface"] == SURFACE_V2
