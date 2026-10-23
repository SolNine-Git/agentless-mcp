"""Write the mcp crate's fixtures: the reference server's argument validation and its answers.

Development-time only. Run it from the repository root with scratch caches:

    XDG_CACHE_HOME=<scratch>/cache TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy> \
        TIKTOKEN_CACHE_DIR=<scratch copy> .venv/bin/python scripts/mcp_fixtures.py

validate.json holds the section cases. Each case is a tool, the arguments of one call, and either
the values that the server's argument validation hands the tool ({"ok": ...}) or the text of its
refusal ({"error": ...}). The text comes from the server through an in-process client, and the
values from a recorder with the tool's own signature. The script refuses to write unless the two
agree. JSON integers outside 64 bits are left out: the Rust server reads them as floats.

calls.json holds the section runs. Each run builds a sandbox by steps, starts the server with its
roots, and makes its calls in order. A call records the text of the answer and its error flag,
with the sandbox and the build as placeholders. A step between two calls changes the sandbox.

With --check, the script compares its output with the committed fixtures and writes nothing.
"""

import asyncio
import inspect
import logging
import os
import sys
import tempfile
from collections.abc import Callable
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Any, get_type_hints
from unittest import mock

import cli_fixtures
from fastmcp import Client, FastMCP
from fastmcp.server.dependencies import without_injected_parameters
from fastmcp.tools import FunctionTool
from fixture_files import Fixture, refuse_unready_environment, write_or_check
from mcp.types import TextContent
from pydantic import TypeAdapter, ValidationError

from agentless_mcp.adapters.mcp import cliargs, server
from agentless_mcp.adapters.mcp.cliargs import SURFACE_V2
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.history_service import HistoryService
from agentless_mcp.application.map_service import MapService
from agentless_mcp.application.repo_context import resolved_allowlist
from agentless_mcp.application.symbol_service import SymbolService
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.bootstrap import select_counter
from agentless_mcp.core.extractor import TreeSitterExtractor

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates" / "mcp" / "fixtures"
REPO_FIXTURES = ROOT / "tests" / "characterization" / "fixtures"
SANDBOX = "<SANDBOX>"
# The first line of a validation refusal holds this; no other answer's first line does.
VALIDATION_HEAD = " validation error"

Arguments = dict[str, object]
Call = tuple[str, Arguments | None]
Step = list[str]


@dataclass(frozen=True)
class Run:
    """One server: the steps that build its sandbox, its roots, and its calls and edits."""

    name: str
    steps: list[Step]
    roots: list[str]
    roots_files: list[str]
    launch: str | None
    calls: list[Call | Step]


def services() -> server.ServerServices:
    """Return the services that the server entry point wires, with the default counter."""
    extractor = TreeSitterExtractor()
    counter = select_counter(None)
    return server.ServerServices(
        maps=MapService(extractor, counter),
        views=ViewService(extractor),
        symbols=SymbolService(extractor, counter),
        graphs=GraphService(extractor),
        histories=HistoryService(extractor, counter),
        counter=counter,
        extractor=extractor,
    )


def built_server(roots: list[Path], roots_files: list[Path], launch: Path | None) -> FastMCP[None]:
    """Return the v2 server that serve() builds for these roots, with no background index."""
    handlers = server.ToolHandlers(
        resolved_allowlist(roots),
        services(),
        roots_files=[cliargs.roots_file(str(path)) for path in roots_files],
        auto_index=False,
        launch_dir=launch,
    )
    return server.build_server(handlers, surface=SURFACE_V2)


async def answers(mcp: FastMCP[None], calls: list[Call]) -> list[tuple[str, bool]]:
    """Return the text and the error flag of each call, made in order in one client session."""
    found = []
    async with Client(mcp) as client:
        for name, arguments in calls:
            result = await client.call_tool_mcp(name, arguments)
            content = result.content[0]
            assert isinstance(content, TextContent)
            found.append((content.text, bool(result.isError)))
    return found


def recorder(name: str, tool: Callable[..., object]) -> TypeAdapter[Any]:
    """Return a validator with the tool's signature whose call returns the validated values."""
    wrapped = without_injected_parameters(tool)

    def record(**values: object) -> dict[str, object]:
        return values

    record.__signature__ = inspect.signature(wrapped)
    record.__annotations__ = get_type_hints(wrapped, include_extras=True)
    record.__name__ = name
    record.__qualname__ = name
    return TypeAdapter(record)


def plain(value: object) -> object:
    """Return a validated value as JSON data: an enum member as its value."""
    if isinstance(value, Enum):
        return value.value
    if isinstance(value, list):
        return [plain(item) for item in value]
    return value


INTS: list[object] = [
    None,
    True,
    False,
    0,
    1,
    -1,
    2,
    7,
    500,
    501,
    2.0,
    2.5,
    -0.0,
    7.0000001,
    1e20,
    1e300,
    9223372036854775807,
    -9223372036854775808,
    18446744073709551615,
    "3",
    " 3 ",
    "\t3\n",
    "+3",
    "-3",
    "3.0",
    "3.00",
    "3.",
    ".5",
    "3.5",
    "1_0",
    "1__0",
    "_10",
    "10_",
    "0x10",
    "0o7",
    "1e3",
    "",
    " ",
    chr(0x663),
    chr(0xFF13),
    "inf",
    "nan",
    "00012",
    "-0",
    "0.0",
    "-0.0",
    "3 4",
    chr(0xA0) + "3",
    "9" * 80,
    "9" * 5000,
    "9" * 4300,
    "9" * 4301,
    "+" + "9" * 4299,
    "1" + "0" * 30 + ".0",
    "-_1",
    "+_1",
    "1_.0",
    "1._0",
    "1.0_0",
    "1_0.0",
    "\x1c3",
    "3\x1f",
    chr(0x3000) + "3",
    [],
    [1],
    {},
    {"a": 1},
]
FLOATS: list[object] = [
    None,
    True,
    False,
    0,
    0.0,
    -0.0,
    1,
    1.5,
    100,
    100.0,
    100.5,
    101,
    1e-300,
    5e-324,
    1e300,
    0.1,
    "1.5",
    " 1.5 ",
    "1e2",
    "1E2",
    "inf",
    "-inf",
    "nan",
    "infinity",
    "0",
    "100",
    "100.0000001",
    "abc",
    "",
    "1_0.5",
    "1_.5",
    "1._5",
    "1e_3",
    "1e1_0",
    "_1.5",
    "1.5_",
    "-_1.5",
    "1__0.5",
    "1.2_5",
    " 1_0 ",
    "1_0 ",
    "\t1_5\n",
    " 10 ",
    "1_5",
    "i_nf",
    "\x1c1.5",
    "0x1p3",
    "+1.5",
    ".5",
    "5.",
    chr(0x661) + ".5",
    [],
    {},
    [1.5],
]
BOOLS: list[object] = [
    None,
    True,
    False,
    0,
    1,
    2,
    -1,
    0.0,
    1.0,
    1.5,
    "true",
    "false",
    "True",
    "FALSE",
    "yes",
    "no",
    "on",
    "off",
    "1",
    "0",
    "t",
    "f",
    "y",
    "n",
    "Yes",
    "YES",
    " true",
    "true ",
    "maybe",
    "",
    "2",
    "0.0",
    [],
    {},
    [True],
]
STRS: list[object] = [
    None,
    True,
    0,
    1.5,
    "",
    " ",
    "map",
    "x" * 60,
    "a'b",
    'a"b',
    "a'b\"c",
    "line\nbreak",
    "tab\t",
    "\x00",
    "\x1f\x7f",
    "\x80\x9f\xa0",
    chr(0xE9),
    chr(0x65E5) + chr(0x672C),
    chr(0x200B),
    chr(0x2028),
    "\U0001f600",
    "\\",
    [],
    ["map"],
    {},
    {"a": [1, 2]},
]
LITERALS: list[object] = [
    None,
    "function",
    "file",
    "body",
    "FILE",
    " file",
    "file ",
    "line",
    "",
    5,
    True,
    [],
    ["file"],
]
KINDS: list[object] = [
    None,
    "class",
    "function",
    "method",
    "constant",
    "type_alias",
    "protocol",
    "dataclass",
    "enum",
    "decorator",
    "FUNCTION",
    "Function",
    " class",
    "widget",
    "",
    5,
    True,
    ["class"],
    {},
]
LISTS: list[object] = [
    None,
    [],
    ["a"],
    ["a", "b"],
    ["a", 1],
    [None],
    [["a"]],
    [True, 2.5],
    "a",
    "",
    5,
    True,
    {},
    {"a": 1},
    ["x" * 40, "y" * 40],
]
FOCI: list[object] = [
    None,
    "",
    "a",
    [],
    ["a"],
    ["a", "b"],
    ["a", 5],
    [5, 6],
    5,
    True,
    {},
    1.5,
    [None],
    [[]],
]
LINES: list[object] = [
    None,
    [],
    [[1, 2]],
    [[1, 1]],
    [[2, 1]],
    [[1]],
    [[1, 2, 3]],
    [[]],
    [[0, 2]],
    [[1, 0]],
    [[1, "x"]],
    [["1", "2"]],
    [[1.0, 2.0]],
    [[1.5, 2]],
    [[True, 2]],
    [[None, 1]],
    [[1, 2], [3]],
    [[1, 2], [0, 0]],
    "x",
    [1],
    [[1, 2], [3, 4], [5, 6]],
    [[-1, -2]],
    [[" 1", "2 "]],
    [["9" * 30, 1]],
    {"a": 1},
    [[1, 2], "x"],
    [[[1], 2]],
    [[1, 2, 3, 4, 5]],
    [[0]],
    [[1, 18446744073709551615]],
    [[0, 0, 0]],
    [[1, 2, "x"]],
    [["x", 1, 2]],
]
# Inputs whose repr each refusal shows: quotes, escapes, the 50-character cut, nested values.
REPRS: list[object] = [
    "a'b",
    'a"b',
    "a'b\"c",
    "x\ny\r\tz",
    "\x00\x1f\x7f\x80\x9f\xa0\xad",
    chr(0xE9) + chr(0xDF),
    chr(0x65E5) + chr(0x672C) + chr(0x8A9E),
    chr(0x2028) + chr(0x2029) + chr(0x200B) + chr(0xFEFF),
    "\U0001f600",
    chr(0xD7FF) + chr(0xE000) + chr(0xFFFF),
    "a" * 47,
    "a" * 48,
    "a" * 49,
    "a" * 50,
    chr(0xE9) * 24,
    chr(0xE9) * 25,
    "a" + chr(0xE9) * 30,
    chr(0xE9) * 60,
    chr(0x65E5) * 30,
    "\U0001f600" * 30,
    [1] * 30,
    {"k" * 10: "v" * 50},
    1.5,
    1e16,
    1e-5,
    1e-4,
    0.1,
    -0.0,
    1e300,
    1.0,
    100.0,
    123456789012345678,
    True,
    False,
    None,
    [],
    {},
    [1.0, "a", None, True, {}],
    {"a": {"b": [1, {"c": "d"}]}},
    [[[[]]]],
    {"": ""},
    {"a'": 'b"'},
]

# An argument set for each tool that names its operation or target.
BASE: dict[str, Arguments] = {
    "orient": {"operation": "cycles"},
    "symbols": {"operation": "find"},
    "read": {"operation": "dir"},
    "find_referencing_symbols": {"target": "t"},
    "history": {"target": "t"},
    "capabilities": {},
}
SWEEPS: list[tuple[str, str, list[object]]] = [
    ("orient", "limit", INTS),
    ("orient", "resolution", FLOATS),
    ("orient", "no_cache", BOOLS),
    ("orient", "include_unique", BOOLS),
    ("find_referencing_symbols", "shared_callers", BOOLS),
    ("read", "whole_file", BOOLS),
    ("symbols", "docstrings", BOOLS),
    ("orient", "operation", STRS),
    ("orient", "repo_root", STRS),
    ("find_referencing_symbols", "target", STRS),
    ("orient", "granularity", LITERALS),
    ("symbols", "kind", KINDS),
    ("symbols", "paths", LISTS),
    ("symbols", "stable_ids", LISTS),
    ("orient", "focus", FOCI),
    ("read", "lines", LINES),
]
BOUNDS: list[tuple[str, str, int, int]] = [
    ("orient", "budget", 200, 64000),
    ("orient", "max_nodes", 1, 500),
    ("orient", "max_edges", 0, 500),
    ("symbols", "limit", 1, 500),
    ("symbols", "context_lines", 0, 200),
    ("read", "context_lines", 0, 200),
    ("read", "depth", 1, 20),
    ("read", "max_entries", 1, 20000),
    ("find_referencing_symbols", "limit", 1, 500),
    ("history", "limit", 1, 500),
    ("history", "budget", 200, 64000),
]
CALL_LEVEL: list[Call] = [
    ("orient", {"limit": 0, "bogus": 1}),
    ("orient", {"zz": 1, "aa": 2, "operation": "map"}),
    ("orient", {"bogus": 1, "operation": 5, "limit": "x", "focus": 5, "resolution": 0}),
    ("orient", {"focus": "x" * 100, "limit": 0}),
    ("orient", {"no_cache": None, "operation": None}),
    ("symbols", {"name": 5, "kind": "x", "paths": "p", "limit": 0, "context_lines": -1}),
    ("read", {"operation": "slice", "lines": [[0, "x"], [3]], "depth": 0, "whole_file": 2}),
    ("history", {"target": None, "limit": "many", "budget": 1}),
    ("find_referencing_symbols", {"limit": 1}),
    ("capabilities", {"repo_root": 5, "extra": []}),
    ("capabilities", {"repo_root": None}),
]


def bound_inputs(low: int, high: int) -> list[object]:
    """Return the inputs at and past each bound, as numbers and as strings."""
    return [low - 1, low, high, high + 1, str(low - 1), str(high + 1), float(high), high + 0.5]


def validation_calls() -> list[Call]:
    """Return every argument set of the validation sweep, in order."""
    calls: list[Call] = [(tool, {}) for tool in BASE]
    for tool, name, inputs in SWEEPS:
        calls.extend((tool, {**BASE[tool], name: value}) for value in inputs)
    for tool, name, low, high in BOUNDS:
        calls.extend((tool, {**BASE[tool], name: value}) for value in bound_inputs(low, high))
    calls.extend(("orient", {**BASE["orient"], "bogus": value}) for value in REPRS)
    return calls + CALL_LEVEL


async def validation_cases() -> list[object]:
    """Return each validation case with the server's answer, checked against the recorder."""
    with tempfile.TemporaryDirectory() as made:
        mcp = built_server([Path(made)], [], None)
        validators: dict[str, TypeAdapter[Any]] = {}
        for listed in await mcp.list_tools():
            registered = await mcp.get_tool(listed.name)
            assert isinstance(registered, FunctionTool)
            validators[listed.name] = recorder(listed.name, registered.fn)
        calls = validation_calls()
        found = await answers(mcp, calls)
    return [
        case(name, arguments or {}, validators[name], answer)
        for (name, arguments), answer in zip(calls, found, strict=True)
    ]


def case(
    name: str, arguments: Arguments, validator: TypeAdapter[Any], answer: tuple[str, bool]
) -> list[object]:
    """Return one validation case, or exit when the recorder and the server disagree."""
    text, error = answer
    refused = error and VALIDATION_HEAD in text.split("\n", 1)[0]
    try:
        values = validator.validate_python(arguments)
    except ValidationError as exc:
        if not refused or str(exc) != text:
            sys.exit(f"refused: {name} {arguments!r}: the server answered {text!r}, not {exc}")
        return [name, arguments, {"error": text}]
    if refused:
        sys.exit(f"refused: {name} {arguments!r}: the server refused what the recorder accepts")
    return [name, arguments, {"ok": {key: plain(value) for key, value in values.items()}}]


def validate_fixture() -> Fixture:
    """Return the validation cases."""
    return {"cases": asyncio.run(validation_cases())}


def file_steps(source: Path, place: str) -> list[Step]:
    """Return a write step for each file below ``source``, under ``place`` in the sandbox."""
    files = sorted(path for path in source.rglob("*.py") if "__pycache__" not in path.parts)
    return [
        ["write", f"{place}/{path.relative_to(source).as_posix()}", path.read_text("utf-8")]
        for path in files
    ]


def repo_steps(place: str, files: dict[str, str]) -> list[Step]:
    """Return the steps that write ``files`` under ``place`` and commit them as a repository."""
    writes = [["write", f"{place}/{name}", text] for name, text in files.items()]
    return [["init", place], *writes, ["commit", place]]


PROBE_FILES = {
    "main.py": "from pkg.mod import alpha\n\nalpha()\n",
    "pkg/mod.py": "def alpha():\n    return beta()\n\n\ndef beta():\n    return 1\n",
}
# The tool calls of the stdio probe captures, in their order.
PROBE_CALLS: list[Call | Step] = [
    ("orient", {"operation": "bogus"}),
    ("orient", {"operation": "map", "source": "x", "target": "y"}),
    ("orient", {"operation": "path"}),
    (
        "orient",
        {
            "operation": "path",
            "source": "",
            "target": "  ",
            "max_nodes": None,
            "include_unique": False,
        },
    ),
    ("orient", {"operation": "map", "limit": 0}),
    ("orient", {"operation": "map", "limit": 501}),
    ("orient", {"operation": "map", "limit": 300}),
    ("orient", {"operation": "map", "bogus": 1}),
    ("orient", {"operation": "map", "focus": 5}),
    ("orient", {"operation": "cycles", "limit": "3"}),
    ("orient", {}),
    ("repo_map", {}),
    ("orient", {"operation": "cycles", "repo_root": "/tmp"}),
    ("read", {"operation": "slice", "path": "main.py", "lines": [[3, 1]]}),
    ("read", {"operation": "slice", "path": "main.py", "lines": [[str(2**64), str(2**64 - 1)]]}),
    ("read", {"operation": "slice", "path": "main.py", "lines": [["9" * 29, "8" * 29]]}),
    ("read", {"operation": "slice", "path": "main.py"}),
    ("read", {"operation": "slice", "path": "main.py", "lines": [[1, 2]], "whole_file": True}),
    ("read", {"operation": "slice", "path": "main.py", "lines": [[1, 2]]}),
    ("symbols", {"operation": "find", "name": "alpha", "kind": "widget"}),
    ("symbols", {"operation": "find", "name": "alpha"}),
    ("orient", {"operation": "map"}),
    ("read", {"operation": "dir"}),
    ("capabilities", {}),
    ("orient", {"operation": "cycles", "limit": 2.0}),
    ("orient", {"operation": "cycles", "limit": 2.5}),
    ("orient", {"operation": "cycles", "limit": True}),
    ("orient", {"operation": "communities", "resolution": 0}),
    ("capabilities", None),
    ("read", {"operation": "slice", "path": "main.py", "lines": [[1]]}),
    ("read", {"operation": "slice", "path": "main.py", "lines": [[0, 2], [1, "x"]]}),
    ("read", {"operation": "slice", "path": "main.py", "lines": []}),
    ("orient", {"operation": "map", "granularity": "line"}),
    ("orient", {"operation": "map", "no_cache": "yes"}),
    ("orient", {"operation": "map", "no_cache": "maybe"}),
    ("orient", {"operation": "map", "budget": "9" * 80}),
    ("orient", {"operation": {"a": [1, 2]}}),
    ("orient", {"operation": None}),
    ("orient", None),
    ("orient", {"operation": "map", "budget": "300.0"}),
    ("orient", {"operation": "map", "budget": " 300 "}),
    ("orient", {"operation": "communities", "resolution": "1.5"}),
    (
        "symbols",
        {"operation": "locate", "path": "main.py", "locations": ["line:1"], "context_lines": 0},
    ),
    ("symbols", {"operation": "find", "name": "alpha", "kind": "FUNCTION"}),
    ("symbols", {"operation": "overview", "paths": "main.py"}),
]


def rich_steps() -> list[Step]:
    """Return the steps of a repository with three commits, an import cycle and packages."""
    first = [["init", "repo"], *file_steps(REPO_FIXTURES / "repo_py", "repo")]
    nested = file_steps(REPO_FIXTURES / "repo_nested", "repo/nested")
    cycle = [
        ["write", "repo/cyc/a.py", "from cyc.b import g\n\n\ndef f():\n    return g()\n"],
        ["write", "repo/cyc/b.py", "from cyc.a import f\n\n\ndef g():\n    return f()\n"],
    ]
    extra = "def surcharge(amount):\n    return amount * {}\n"
    later = [
        ["write", "repo/pricing_extra.py", extra.format(2)],
        ["commit", "repo", "add a surcharge"],
        ["write", "repo/pricing_extra.py", extra.format(3)],
        ["commit", "repo", "raise the surcharge"],
    ]
    return [*first, *nested, *cycle, ["commit", "repo", "first"], *later]


EXPANDED = ["py:pricing.py::PriceBook.cost_of", "py:pricing.py::no_such", "py:missing.py::f"]
LOCATIONS = ["class:PriceBook", "function:cost_of", "line:3", "bogus:x"]
RICH_CALLS: list[Call | Step] = [
    ("orient", {"operation": "map"}),
    ("orient", {"operation": "map", "focus": "reports.py"}),
    ("orient", {"operation": "map", "focus": ["pricing.py", "inventory"], "limit": 2}),
    ("orient", {"operation": "map", "focus": "", "budget": 200}),
    ("orient", {"operation": "map", "granularity": "file", "limit": 1}),
    ("orient", {"operation": "map", "granularity": "body", "budget": 400}),
    ("orient", {"operation": "map", "granularity": "function", "no_cache": True}),
    ("orient", {"operation": "communities"}),
    ("orient", {"operation": "communities", "resolution": 0.5, "limit": 1}),
    ("orient", {"operation": "cycles"}),
    ("orient", {"operation": "cycles", "limit": 1}),
    ("orient", {"operation": "health"}),
    ("orient", {"operation": "health", "limit": 1}),
    ("orient", {"operation": "diagram"}),
    ("orient", {"operation": "diagram", "focus": ["reports.py", "x"], "max_nodes": 2}),
    ("orient", {"operation": "diagram", "max_edges": 0, "group_by_communities": True}),
    ("orient", {"operation": "diagram", "focus": "", "resolution": 2}),
    ("orient", {"operation": "path", "source": "reports.py", "target": "pricing.py"}),
    (
        "orient",
        {
            "operation": "path",
            "source": "summarise",
            "target": "format_money",
            "include_unique": True,
            "include_ambiguous": True,
        },
    ),
    ("orient", {"operation": "path", "source": "nothing_here", "target": "pricing.py"}),
    ("orient", {"operation": "map", "include_unique": False, "max_edges": 0, "focus": []}),
    ("orient", {"operation": "cycles", "limit": 3, "group_by_communities": True}),
    ("symbols", {"operation": "find", "name": "price"}),
    ("symbols", {"operation": "find", "name": "price", "kind": "class", "limit": 1}),
    ("symbols", {"operation": "find", "name": "zzz_no_such"}),
    ("symbols", {"operation": "find", "name": "zzz_no_such", "kind": "enum"}),
    ("symbols", {"operation": "overview", "paths": ["pricing.py", "reports.py"]}),
    (
        "symbols",
        {
            "operation": "overview",
            "paths": ["pricing.py", "missing.py", "../out.py"],
            "docstrings": True,
        },
    ),
    ("symbols", {"operation": "overview", "paths": ["inventory.py"], "docstrings": False}),
    ("symbols", {"operation": "overview", "paths": []}),
    ("symbols", {"operation": "expand", "stable_ids": EXPANDED}),
    ("symbols", {"operation": "expand", "stable_ids": EXPANDED[:1] * 2, "limit": 1}),
    ("symbols", {"operation": "explain", "target": "format_money"}),
    ("symbols", {"operation": "explain", "target": "py:pricing.py::PriceBook", "limit": 1}),
    ("symbols", {"operation": "explain", "target": "zzz_no_such"}),
    ("symbols", {"operation": "locate", "path": "pricing.py", "locations": LOCATIONS}),
    (
        "symbols",
        {
            "operation": "locate",
            "path": "pricing.py",
            "locations": ["line:999"],
            "context_lines": 2,
        },
    ),
    ("symbols", {"operation": "find", "name": "x", "paths": ["a"], "docstrings": False}),
    ("symbols", {"operation": "expand"}),
    ("read", {"operation": "slice", "path": "pricing.py", "lines": [[1, 3], [10, 12]]}),
    ("read", {"operation": "slice", "path": "pricing.py", "lines": [[1, 2]], "context_lines": 0}),
    ("read", {"operation": "slice", "path": "cyc/a.py", "whole_file": True}),
    ("read", {"operation": "slice", "path": "pricing.py", "lines": [[500, 510]]}),
    ("read", {"operation": "slice", "path": "missing.py", "whole_file": True}),
    ("read", {"operation": "slice", "path": "../outside.py", "whole_file": True}),
    ("read", {"operation": "slice", "path": "nested", "whole_file": True}),
    ("read", {"operation": "slice", "lines": [[1, 2]]}),
    ("read", {"operation": "dir"}),
    ("read", {"operation": "dir", "path": ""}),
    ("read", {"operation": "dir", "path": "nested"}),
    ("read", {"operation": "dir", "path": "nested/billing", "depth": 1}),
    ("read", {"operation": "dir", "path": "pricing.py"}),
    ("read", {"operation": "dir", "path": "no_such_dir"}),
    ("read", {"operation": "dir", "path": "/"}),
    ("read", {"operation": "dir", "path": "../repo/nested"}),
    ("read", {"operation": "dir", "depth": 1, "max_entries": 2}),
    ("read", {"operation": "dir", "path": "  "}),
    ("read", {"operation": "dir", "whole_file": True}),
    ("find_referencing_symbols", {"target": "format_money"}),
    ("find_referencing_symbols", {"target": "py:pricing.py::PriceBook", "limit": 1}),
    ("find_referencing_symbols", {"target": "format_money", "shared_callers": True}),
    ("find_referencing_symbols", {"target": "zzz_no_such"}),
    ("history", {"target": "py:pricing_extra.py::surcharge"}),
    ("history", {"target": "py:pricing_extra.py::surcharge", "limit": 1, "budget": 200}),
    ("history", {"target": "py:pricing.py::no_such"}),
    ("history", {"target": "surcharge"}),
    ("capabilities", {}),
    ("capabilities", {"repo_root": f"{SANDBOX}/repo"}),
    ("orient", {"operation": "map", "repo_root": f"{SANDBOX}/repo/"}),
    ("orient", {"operation": "map", "repo_root": f"{SANDBOX}/repo/nested"}),
    ("orient", {"operation": "map", "repo_root": "  "}),
]
TWO = {
    "a.py": "def a():\n    return 1\n",
    "b/c.py": "from a import a\n\n\ndef c():\n    return a()\n",
}
NOT_GIT_CALLS: list[Call | Step] = [
    ("orient", {"operation": "map"}),
    ("history", {"target": "py:a.py::a"}),
    ("read", {"operation": "dir"}),
    ("capabilities", {}),
]


def several_calls() -> list[Call | Step]:
    """Return the calls of a server with several roots."""
    return [
        ("orient", {"operation": "map"}),
        ("capabilities", {}),
        ("orient", {"operation": "map", "repo_root": f"{SANDBOX}/one"}),
        ("orient", {"operation": "map", "repo_root": f"{SANDBOX}/two"}),
        ("orient", {"operation": "map", "repo_root": f"{SANDBOX}/one/inner"}),
        ("capabilities", {"repo_root": f"{SANDBOX}/two"}),
        ("orient", {"operation": "map", "repo_root": f"{SANDBOX}/elsewhere"}),
        ("read", {"operation": "dir", "repo_root": "relative/path"}),
        ("orient", {"operation": "map", "repo_root": "~/x"}),
        ("orient", {"operation": "map", "repo_root": "~nosuchuser_zz/x"}),
        ("orient", {"operation": "map", "repo_root": "~root/x"}),
        ("orient", {"operation": "map", "repo_root": "a\x00b"}),
    ]


def no_root_calls() -> list[Call | Step]:
    """Return the calls of a server with no roots."""
    return [
        ("orient", {"operation": "map"}),
        ("orient", {"operation": "map", "repo_root": f"{SANDBOX}/x"}),
        ("capabilities", {}),
    ]


def roots_file_calls() -> list[Call | Step]:
    """Return the calls of a server with a roots file, and the edits between them."""
    one, two, inner = f"{SANDBOX}/one", f"{SANDBOX}/two", f"{SANDBOX}/one/inner"
    return [
        ("orient", {"operation": "map"}),
        ("orient", {"operation": "map", "repo_root": two}),
        ("orient", {"operation": "map", "repo_root": inner}),
        ["step", "roots", "roots.txt", two, inner, "one/inner/../inner"],
        ("orient", {"operation": "map", "repo_root": inner}),
        ("capabilities", {"repo_root": one}),
        ["step", "roots", "roots.txt", chr(0xFEFF) + inner],
        ("orient", {"operation": "map", "repo_root": two}),
        ("orient", {"operation": "map", "repo_root": inner}),
        ["step", "bytes", "roots.txt", "\xff\xfe"],
        ("orient", {"operation": "map", "repo_root": one}),
        ["step", "remove", "roots.txt"],
        ("orient", {"operation": "map", "repo_root": one}),
        ("capabilities", {}),
    ]


def runs() -> list[Run]:
    """Return every run of the calls fixture."""
    two = [*repo_steps("one", TWO), *repo_steps("two", TWO), *repo_steps("one/inner", TWO)]
    roots_file = [*two, ["roots", "roots.txt", "# served", "", f"  {SANDBOX}/two  "]]
    plain_dir = [["write", "plain/a.py", "def a():\n    return 1\n"]]
    no_root = [["write", "x/a.py", "x = 1\n"]]
    return [
        Run("probe", repo_steps("repo", PROBE_FILES), ["repo"], [], "repo", PROBE_CALLS),
        Run("rich", rich_steps(), ["repo"], [], "repo", RICH_CALLS),
        Run("several_roots", two, ["one", "two"], [], None, several_calls()),
        Run("nested_roots", two, ["one", "one/inner"], [], None, several_calls()),
        Run("no_roots", no_root, [], [], None, no_root_calls()),
        Run("roots_file", roots_file, ["one"], ["roots.txt"], None, roots_file_calls()),
        Run("not_git", plain_dir, ["plain"], [], "plain", NOT_GIT_CALLS),
    ]


def apply(sandbox: Path, env: dict[str, str], step: Step) -> None:
    """Run one step in ``sandbox``: a roots file, raw bytes, a removal, or a cli fixture step."""
    kind, place, *rest = step
    if kind == "roots":
        lines = [line.replace(SANDBOX, str(sandbox)) for line in rest]
        (sandbox / place).write_text("".join(f"{line}\n" for line in lines), encoding="utf-8")
    elif kind == "bytes":
        # The file holds one byte per character of the text, so it need not be UTF-8.
        (sandbox / place).write_bytes(rest[0].encode("latin-1"))
    elif kind == "remove":
        (sandbox / place).unlink()
    else:
        cli_fixtures.apply(sandbox, env, step)


def placed(value: object, sandbox: Path) -> object:
    """Return ``value`` with each sandbox placeholder in its strings replaced by the sandbox."""
    if isinstance(value, str):
        return value.replace(SANDBOX, str(sandbox))
    if isinstance(value, dict):
        return {key: placed(item, sandbox) for key, item in value.items()}
    if isinstance(value, list):
        return [placed(item, sandbox) for item in value]
    return value


def replayed(spec: Run) -> dict[str, object]:
    """Return the run with the text and the error flag of each call's answer."""
    with tempfile.TemporaryDirectory(prefix="mcp-fixture-") as made:
        sandbox = Path(made).resolve()
        (sandbox / "home").mkdir()
        env = cli_fixtures.environment(sandbox, None)
        with mock.patch.dict(os.environ, env, clear=True):
            calls = replay_in(spec, sandbox, env)
    return {
        "name": spec.name,
        "steps": spec.steps,
        "roots": spec.roots,
        "roots_files": spec.roots_files,
        "launch": spec.launch,
        "calls": calls,
    }


def replay_in(spec: Run, sandbox: Path, env: dict[str, str]) -> list[object]:
    """Build the run's sandbox and server, then make its calls with its edits between them."""
    for step in spec.steps:
        apply(sandbox, env, step)
    launch = None if spec.launch is None else sandbox / spec.launch
    mcp = built_server(
        [sandbox / root for root in spec.roots], [sandbox / f for f in spec.roots_files], launch
    )
    cwd = Path.cwd()
    os.chdir(sandbox)
    try:
        return [one_call(mcp, item, sandbox, env) for item in spec.calls]
    finally:
        os.chdir(cwd)


def one_call(mcp: FastMCP[None], item: Call | Step, sandbox: Path, env: dict[str, str]) -> object:
    """Return a step after running it, or a call with its normalized answer."""
    if isinstance(item, list):
        apply(sandbox, env, item[1:])
        return item
    name, arguments = item
    given = placed(arguments, sandbox)
    assert given is None or isinstance(given, dict)
    [(text, error)] = asyncio.run(answers(mcp, [(name, given)]))
    return [name, arguments, cli_fixtures.normalised(text, sandbox), error]


def calls_fixture() -> Fixture:
    """Return every run with its answers."""
    return {"runs": [replayed(spec) for spec in runs()]}


def main() -> int:
    """Write or check the mcp fixtures."""
    refuse_unready_environment()
    # The reference server logs each refused call on stderr; the fixtures hold the answers only.
    logging.disable(logging.CRITICAL)
    return write_or_check(
        "Write the mcp crate's fixtures.",
        FIXTURES,
        {"validate": validate_fixture, "calls": calls_fixture},
    )


if __name__ == "__main__":
    sys.exit(main())
