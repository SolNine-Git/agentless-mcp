"""Write the app crate's surface fixture: the v2 tool entries, operation tables and unlock rule.

Development-time only. Run it from the repository root with scratch caches:

    TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy> TIKTOKEN_CACHE_DIR=<scratch copy> \
        uv run python scripts/surface_fixtures.py

surface.json holds three sections. tools: each tool entry that a client lists from a v2 server, in
list order. operations: each row of the v2 operation tables, as the tool, the operation, and the
parameters that it accepts and requires. unlock: for each v2 tool, each of its operations, no
operation and an unknown operation, the reason that the gate-mark hook records, or null when the
call does not unlock the search gate. With --check, the script compares its output with the
committed fixtures and writes nothing.
"""

import asyncio
import importlib.util
import sys
import tempfile
from collections.abc import Callable, Mapping
from pathlib import Path
from typing import Any, cast

from fastmcp import Client
from fixture_files import Fixture, refuse_unready_environment, write_or_check

from agentless_mcp.adapters.mcp import server
from agentless_mcp.adapters.mcp.cliargs import SURFACE_V2
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.history_service import HistoryService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.bootstrap import select_counter
from agentless_mcp.core.extractor import TreeSitterExtractor

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates" / "app" / "fixtures" / "surface"
GATE_MARK = ROOT / "contrib" / "hooks" / "agentless_gate_mark.py"
# The prefix that Claude Code puts on the tools of the server it names "agentless".
TOOL_PREFIX = "mcp__agentless__"
UNKNOWN_OPERATION = "no_such_operation"
OPERATION_TABLES: Mapping[str, Mapping[str, server.OperationSpec]] = {
    "orient": server.ORIENT_OPERATIONS,
    "symbols": server.SYMBOLS_OPERATIONS,
    "read": server.READ_OPERATIONS,
}

UnlockReason = Callable[[dict[str, object]], str | None]


def listed_tools() -> list[dict[str, Any]]:
    """Return each v2 tool entry as a client lists it, in list order."""
    extractor = TreeSitterExtractor()
    counter = select_counter(None)
    services = server.ServerServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor, counter),
        graphs=GraphService(extractor),
        histories=HistoryService(extractor, counter),
        counter=counter,
        extractor=extractor,
    )
    with tempfile.TemporaryDirectory() as root:
        handlers = server.ToolHandlers([Path(root)], services)
        mcp = server.build_server(handlers, surface=SURFACE_V2)

        async def listing() -> list[Any]:
            async with Client(mcp) as client:
                return list(await client.list_tools())

        tools = asyncio.run(listing())
    return [tool.model_dump(by_alias=True, exclude_none=True, mode="json") for tool in tools]


def operation_cases(names: list[str]) -> list[object]:
    """Return each operation table row of the listed tools, in table order."""
    return [
        [name, operation, list(spec.accepted), list(spec.required)]
        for name in names
        for operation, spec in OPERATION_TABLES.get(name, {}).items()
    ]


def gate_mark_unlock_reason() -> UnlockReason:
    """Return the gate-mark hook's unlock_reason, loaded from the hook file."""
    spec = importlib.util.spec_from_file_location("agentless_gate_mark", GATE_MARK)
    if spec is None or spec.loader is None:
        message = f"{GATE_MARK} cannot be loaded"
        raise SystemExit(message)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return cast("UnlockReason", module.unlock_reason)


def unlock_cases(names: list[str], unlock_reason: UnlockReason) -> list[object]:
    """Return the reason the hook records for each tool and operation, or null for none."""
    cases: list[object] = []
    for name in names:
        table = OPERATION_TABLES.get(name)
        operations = [None] if table is None else [*table, None, UNKNOWN_OPERATION]
        for operation in operations:
            payload: dict[str, object] = {"tool_name": TOOL_PREFIX + name}
            if operation is not None:
                payload["tool_input"] = {"operation": operation}
            cases.append([name, operation, unlock_reason(payload)])
    return cases


def surface_fixture() -> Fixture:
    """Return the three sections of surface.json."""
    tools = listed_tools()
    names = [str(tool["name"]) for tool in tools]
    return {
        "tools": list(tools),
        "operations": operation_cases(names),
        "unlock": unlock_cases(names, gate_mark_unlock_reason()),
    }


def main() -> int:
    """Write the fixture, or compare it with --check, and return the exit status."""
    refuse_unready_environment()
    return write_or_check(__doc__.splitlines()[0], FIXTURES, {"surface": surface_fixture})


if __name__ == "__main__":
    sys.exit(main())
