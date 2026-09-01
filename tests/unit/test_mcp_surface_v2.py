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
import logging
import sqlite3
from dataclasses import dataclass

import pytest
from fastmcp import Client
from fastmcp.exceptions import ToolError

from agentless_mcp.adapters.cli.formatting import EXIT_OK
from agentless_mcp.adapters.cli.main import CliServices
from agentless_mcp.adapters.cli.main import run as cli_run
from agentless_mcp.adapters.mcp import server as server_module
from agentless_mcp.adapters.mcp.cliargs import (
    SURFACE_BOTH,
    SURFACE_V1,
    SURFACE_V2,
    SURFACES,
    parse_args,
)
from agentless_mcp.adapters.mcp.server import (
    ServerServices,
    ToolHandlers,
    build_server,
)
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.lint_service import LintService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.validate_service import ValidateService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.errors import AgentlessError
from agentless_mcp.util.tokens import Chars4Counter

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
        {"operation": "map", "granularity": "body"},
        "repo_map",
        {"granularity": "body"},
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
    ("orient", {"operation": "health"}, "analyze_structure", {"operation": "health"}),
    (
        "orient",
        {"operation": "health", "limit": 2},
        "analyze_structure",
        {"operation": "health", "limit": 2},
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

    def test_no_tool_on_either_surface_publishes_an_output_schema(self, services, one_repo):
        """All fourteen answer in text, so none declares a structured shape.

        Built on ``SURFACE_BOTH`` rather than the default: the default
        registers five tools, and the nine v1 registrations return ``str``
        the same way and carry the same defect if one of them forgets
        ``output_schema=None``.
        """
        tools = listed_tools(build_server(ToolHandlers([one_repo], services), surface=SURFACE_BOTH))

        assert len(tools) == 14
        for tool in tools:
            assert tool.outputSchema is None, tool.name

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

    def test_every_v2_tool_asks_clients_to_always_load_it(self, services, one_repo):
        # Deferral-capable clients keep these five schemas out of context
        # without this hint, and an unloaded schema routes agents to grep.
        tools = listed_tools(build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2))
        for tool in tools:
            assert (tool.meta or {}).get("anthropic/alwaysLoad") is True, tool.name

    def test_v1_only_tools_never_ask_for_always_load(self, services, one_repo):
        tools = listed_tools(build_server(ToolHandlers([one_repo], services), surface=SURFACE_BOTH))
        for tool in tools:
            if tool.name in EXPECTED_TOOLS_V2:
                continue
            meta = tool.meta or {}
            assert "anthropic/alwaysLoad" not in meta, tool.name


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
        [
            "orient has no operation named 'graph'",
            "communities, cycles, diagram, health, map, path",
        ],
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


class TestZeroValuesAreNotStrayParameters:
    """A parameter set to nothing is not a parameter the operation was given.

    A generated call commonly fills every declared optional with a zero value.
    Refusing those as stray refuses a call that asked for nothing unusual --
    and for a flag whose v1 counterpart defaulted to False, it refuses the
    default itself.
    """

    def call_ok(self, services, one_repo, tool, arguments):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)
        return call(server, tool, {"repo_root": str(one_repo), **arguments})

    def test_a_false_flag_is_not_a_stray_parameter(self, services, one_repo):
        self.call_ok(services, one_repo, "read", {"operation": "dir", "whole_file": False})

    def test_a_blank_string_is_not_a_stray_parameter(self, services, one_repo):
        self.call_ok(
            services, one_repo, "orient", {"operation": "map", "source": "", "target": "  "}
        )

    def test_false_booleans_foreign_to_map_are_not_stray(self, services, one_repo):
        self.call_ok(
            services,
            one_repo,
            "orient",
            {"operation": "map", "include_unique": False, "group_by_communities": False},
        )

    def test_an_empty_list_is_not_a_stray_parameter(self, services, one_repo):
        # The rule recognised None, False and blank strings but not an empty
        # sequence, so a generated call that filled every list optional was
        # refused for the ones the operation does not take.
        self.call_ok(
            services, one_repo, "symbols", {"operation": "find", "name": "quote", "paths": []}
        )

    def test_a_zero_number_is_not_a_stray_parameter(self, services, one_repo):
        self.call_ok(services, one_repo, "read", {"operation": "dir", "context_lines": 0})

    def test_a_real_value_is_still_refused_as_stray(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        with pytest.raises(ToolError, match="does not accept: source"):
            call(
                server,
                "orient",
                {"repo_root": str(one_repo), "operation": "map", "source": "quote"},
            )


class TestAnEmptyRequiredListIsMissing:
    """`required=("paths",)` has to mean what it reads as.

    An empty list satisfied the required check, so
    `symbols(operation="overview", paths=[])` answered with a receipt and an
    empty body -- a success that answered nothing, which is the
    substitute-content failure `read` operation 'slice' refuses on principle.
    """

    @pytest.mark.parametrize(
        ("operation", "arguments", "expected"),
        [
            ("overview", {"paths": []}, "is missing: paths"),
            ("expand", {"stable_ids": []}, "is missing: stable_ids"),
        ],
        ids=["overview", "expand"],
    )
    def test_it_is_refused_by_name(self, services, one_repo, operation, arguments, expected):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        with pytest.raises(ToolError, match=expected):
            call(
                server,
                "symbols",
                {"repo_root": str(one_repo), "operation": operation, **arguments},
            )


class TestThePathRefusalNamesTheToolThatRefused:
    """It hardcoded `analyze_structure`, so a v2 caller read a v1 tool name.

    Unreachable from the v2 wire today: `_checked_operation` refuses a blank
    endpoint first and its message already names `orient`. That is exactly why
    the hardcoding was worth removing -- it is a backstop that becomes wrong
    silently the moment v1 is retired or the blank check moves, and neither
    change would fail a test. Driven at the function, which is the level the
    backstop lives at.
    """

    @pytest.mark.parametrize("tool", ["analyze_structure", "orient"])
    def test_the_refusal_names_the_surface_the_caller_used(self, tool):
        request = server_module.StructureRequest(operation="path", tool=tool)

        with pytest.raises(AgentlessError, match=f"{tool} operation 'path' needs both"):
            server_module._operation_path(None, None, request)

    def test_the_v2_wire_is_guarded_before_the_backstop(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        with pytest.raises(ToolError, match="orient operation 'path' is missing: source, target"):
            call(
                server,
                "orient",
                {"repo_root": str(one_repo), "operation": "path", "source": " ", "target": " "},
            )


class TestMapLimitMatchesItsV1Counterpart:
    """orient's map operation carries repo_map's file cap, not the listing cap.

    `limit` is one wire parameter serving three operations, and the ceiling is
    not shared: communities and cycles are listings, map is the repository map
    whose bound `repo_map` publishes as `max_files`.
    """

    def test_a_limit_past_the_v1_cap_is_refused(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        with pytest.raises(ToolError, match="rejects limit=201"):
            call(
                server,
                "orient",
                {"repo_root": str(one_repo), "operation": "map", "limit": 201},
            )

    def test_the_refusal_names_the_range_and_why(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        with pytest.raises(ToolError) as raised:
            call(
                server,
                "orient",
                {"repo_root": str(one_repo), "operation": "map", "limit": 500},
            )

        message = str(raised.value)
        assert "1-200" in message, message
        assert "max_files" in message, message
        assert "validation error" not in message, message

    def test_the_cap_itself_is_accepted(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        call(server, "orient", {"repo_root": str(one_repo), "operation": "map", "limit": 200})

    def test_the_listing_operations_keep_the_wider_ceiling(self, services, one_repo):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        call(
            server, "orient", {"repo_root": str(one_repo), "operation": "communities", "limit": 500}
        )


# Every numeric parameter the v2 surface publishes, at a value below its floor.
OUT_OF_RANGE = [
    ("orient", {"operation": "map", "limit": 0}),
    ("orient", {"operation": "map", "limit": -1}),
    ("orient", {"operation": "cycles", "limit": 0}),
    ("orient", {"operation": "communities", "resolution": 0}),
    ("orient", {"operation": "communities", "resolution": -1}),
    ("orient", {"operation": "diagram", "max_nodes": 0}),
    ("symbols", {"operation": "find", "name": "quote", "limit": 0}),
    ("symbols", {"operation": "find", "name": "quote", "limit": -1}),
    ("read", {"operation": "slice", "path": "core.py", "lines": [[1, 2]], "context_lines": -1}),
    ("find_referencing_symbols", {"target": "quote", "limit": 0}),
]


class TestTheWireRefusesWhatTheCliAccepts:
    """The other half of the bounds story pinned in ``test_cli_bounds.py``.

    Every numeric parameter here carries a ``Field(ge=, le=)``, so zero and
    minus one never reach a service through this door. The CLI spells the
    same parameters as a bare ``type=int`` and lets most of them through, and
    three of its commands answer a bound of zero by reporting that the
    repository is empty. One number, two contracts. Stage 4b gives the
    services one owner so both doors inherit the same answer; until then
    these two modules are what measure the gap.
    """

    @pytest.mark.parametrize(
        ("tool", "arguments"),
        OUT_OF_RANGE,
        ids=[f"{tool}-{sorted(arguments.items())[-1]}" for tool, arguments in OUT_OF_RANGE],
    )
    def test_zero_and_negative_never_reach_a_service(self, services, one_repo, tool, arguments):
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        with pytest.raises(ToolError):
            call(server, tool, {"repo_root": str(one_repo), **arguments})


class TestTheRefusalContractDoesNotDependOnTheEnvironment:
    """Two ways the wording an agent reads could stop being this package's.

    FastMCP reads ``FASTMCP_MASK_ERROR_DETAILS`` from the environment, and a
    masked error replaces a refusal that names the operation, what it accepts
    and what it requires with a generic sentence. Measured before the pin: 243
    of these tests passed with the variable unset and 22 failed with it set.
    Whether an agent can correct its own call is not an operator's setting.

    The other way is the opposite error. With masking off, an exception this
    package did not plan for hands its own text to the client, and those carry
    local detail -- a sqlite failure names the absolute path of the tag cache.
    """

    def test_a_refusal_survives_the_masking_environment_variable(
        self, services, one_repo, monkeypatch
    ):
        monkeypatch.setenv("FASTMCP_MASK_ERROR_DETAILS", "true")
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        with pytest.raises(ToolError) as raised:
            call(server, "orient", {"repo_root": str(one_repo), "operation": "nonsense"})

        message = str(raised.value)
        assert "nonsense" in message
        for operation in ("map", "communities", "cycles", "diagram", "path"):
            assert operation in message

    def test_an_unplanned_failure_does_not_hand_its_own_words_to_the_client(
        self, services, one_repo, monkeypatch, caplog
    ):
        """A defect is reported as a defect, and the detail goes to the log."""
        local_path = "/home/someone/.cache/agentless-mcp/9f2a/tags.db"

        def explode(*args, **kwargs):
            message = f"unable to open database file {local_path}"
            raise sqlite3.OperationalError(message)

        monkeypatch.setattr(services.maps, "build", explode)
        server = build_server(ToolHandlers([one_repo], services), surface=SURFACE_V2)

        with (
            caplog.at_level(logging.ERROR, logger=server_module.logger.name),
            pytest.raises(ToolError) as raised,
        ):
            call(server, "orient", {"repo_root": str(one_repo), "operation": "map"})

        message = str(raised.value)
        assert local_path not in message
        assert "defect in agentless-mcp" in message
        assert local_path in caplog.text


# --------------------------------------------------------------------------
# Two-door parity: one value, both front doors, one verdict.
# --------------------------------------------------------------------------

# The gate the parity claim lacked. `test_front_doors` pins each surface --
# which options exist, which constants are published -- and a surface
# inventory cannot see the defect that motivated it, because both doors
# *declared* `limit` while only one *enforced* a ceiling on it. So
# `cycles --limit 100000` was answered on the command line and refused over
# MCP, for the same repository, and nothing failed.


@dataclass(frozen=True)
class DoorCase:
    """One value, spelled for each door, with the verdict both must reach."""

    name: str
    argv: tuple[str, ...]
    tool: str
    arguments: dict[str, object]
    verdict: str


OUT_OF_RANGE = (
    DoorCase(
        "limit-above",
        ("cycles", "--limit", "100000"),
        "orient",
        {"operation": "cycles", "limit": 100000},
        "refused",
    ),
    DoorCase(
        "limit-zero",
        ("cycles", "--limit", "0"),
        "orient",
        {"operation": "cycles", "limit": 0},
        "refused",
    ),
    DoorCase(
        "resolution-above",
        ("communities", "--resolution", "1000"),
        "orient",
        {"operation": "communities", "resolution": 1000.0},
        "refused",
    ),
    DoorCase(
        "max-nodes-above",
        ("diagram", "--max-nodes", "100000"),
        "orient",
        {"operation": "diagram", "max_nodes": 100000},
        "refused",
    ),
    DoorCase(
        "max-edges-above",
        ("diagram", "--max-edges", "100000"),
        "orient",
        {"operation": "diagram", "max_edges": 100000},
        "refused",
    ),
    DoorCase(
        "depth-above",
        ("tree", "--depth", "999"),
        "read",
        {"operation": "dir", "depth": 999},
        "refused",
    ),
    DoorCase(
        "max-entries-above",
        ("tree", "--max-entries", "999999"),
        "read",
        {"operation": "dir", "max_entries": 999999},
        "refused",
    ),
    DoorCase(
        "context-above",
        ("slice", "core.py", "--context", "999"),
        "read",
        {"operation": "slice", "path": "core.py", "lines": [[1, 2]], "context_lines": 999},
        "refused",
    ),
)

# The mirror: a value inside the range, which both doors must ACCEPT. A parity
# test that only checked refusals would pass equally well against two doors
# that refused everything.
IN_RANGE = (
    DoorCase(
        "limit-at-ceiling",
        ("cycles", "--limit", "500"),
        "orient",
        {"operation": "cycles", "limit": 500},
        "accepted",
    ),
    DoorCase(
        "max-edges-at-floor",
        ("diagram", "--max-edges", "0"),
        "orient",
        {"operation": "diagram", "max_edges": 0},
        "accepted",
    ),
    DoorCase(
        "depth-at-ceiling",
        ("tree", "--depth", "20"),
        "read",
        {"operation": "dir", "depth": 20},
        "accepted",
    ),
    DoorCase(
        "context-at-ceiling",
        ("slice", "core.py", "--context", "200"),
        "read",
        {"operation": "slice", "path": "core.py", "lines": [[1, 2]], "context_lines": 200},
        "accepted",
    ),
)


def cli_verdict(root, argv):
    """Run one CLI invocation in process and say whether it was refused."""
    extractor = TreeSitterExtractor()
    counter = Chars4Counter()
    patches = PatchService(extractor)
    wiring = CliServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor, counter),
        graphs=GraphService(extractor),
        patches=patches,
        validates=ValidateService(patches),
        lints=LintService(extractor),
        counter=counter,
        extractor=extractor,
    )
    return "accepted" if cli_run([*argv, "--repo", str(root)], wiring) == EXIT_OK else "refused"


def mcp_verdict(server, root, case):
    """Call the same parameter over MCP and say whether it was refused."""
    try:
        call(server, case.tool, {"repo_root": str(root), **case.arguments})
    except (ToolError, AgentlessError):
        # A wire-schema rejection reaches the client wrapped in ToolError; a
        # service refusal is this package's own exception.
        return "refused"
    return "accepted"


class TestTheTwoDoorsAgree:
    """A bound is a bound on both doors, or it is a difference between them."""

    @pytest.mark.parametrize("case", [*OUT_OF_RANGE, *IN_RANGE], ids=lambda case: case.name)
    def test_both_doors_reach_the_same_verdict(self, services, one_repo, case):
        server = build_server(ToolHandlers([one_repo], services, auto_index=False), SURFACE_V2)
        assert cli_verdict(one_repo, case.argv) == case.verdict
        assert mcp_verdict(server, one_repo, case) == case.verdict
