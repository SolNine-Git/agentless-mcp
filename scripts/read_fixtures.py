"""Write the read-surface fixtures of the app and cli crates: what the Python build answers.

Development-time only. Run it from the repository root, with the grammar libraries copied to a
scratch directory and the caches inside a scratch directory, so that no run reads or writes the
user's caches:

    XDG_CACHE_HOME=<scratch>/cache TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy>/v1.14.3/libs \
        TIKTOKEN_CACHE_DIR=<scratch copy of the cl100k_base file> \
        .venv/bin/python scripts/read_fixtures.py

The token counts need the 'tokens' extra and the cl100k_base file in TIKTOKEN_CACHE_DIR, so that
tiktoken reads no network.

With --check, the script compares its output with the committed fixtures and writes nothing.

Each file holds one JSON object of named sections, one case per line. The files of this script
live in the read/ subdirectory of each crate's fixtures. The read-surface generators of the other
parts write to their own subdirectories.
"""

import asyncio
import json
import math
import os
import struct
import subprocess
import sys
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest import mock

import cli_fixtures
import tiktoken
from fixture_files import (
    GRAMMAR_OVERRIDE,
    Fixture,
    refuse_unready_environment,
    render,
    write_or_check_files,
)

from agentless_mcp.adapters.mcp import server
from agentless_mcp.application import map_service
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.application.view_service import ViewService
from agentless_mcp.core import refs, treewalk
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.symbols import symbol_stable_id
from agentless_mcp.util import bounds
from agentless_mcp.util.errors import OperationFailed

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "crates" / "app" / "fixtures" / "read"
CLI = ROOT / "crates" / "cli" / "fixtures" / "read"

# Unicode digits: Python's int() reads them and the Rust build refuses them (ledger deviation).
ARABIC_TWELVE = chr(0x0661) + chr(0x0662)
NO_BREAK_SPACE = chr(0x00A0)
LINE_SEPARATOR = chr(0x2028)
INT_TEXTS = [
    "0",
    "12",
    " +0012 ",
    "-0",
    "-7",
    "1_000",
    "1__0",
    "_1",
    "1_",
    "0x10",
    "",
    " ",
    "1.0",
    "1e3",
    "\t7\n",
    NO_BREAK_SPACE + "7" + NO_BREAK_SPACE,
    "9" * 20,
    "-" + "9" * 20,
    "0" * 4299 + "1",
    "0" * 4300 + "1",
    "1" * 4300,
    "1" * 4301,
    "1_" * 2150 + "1",
]
DEVIATION_TEXTS = [ARABIC_TWELVE, "1" + ARABIC_TWELVE]


def bits(x: float) -> str:
    (raw,) = struct.unpack("<Q", struct.pack("<d", x))
    return f"0x{raw:016x}"


def python_int(text: str) -> str | None:
    """The repr of int(text), or None when int() refuses it."""
    try:
        return repr(int(text))
    except ValueError:
        return None


def refusal(rule: Callable[[], object]) -> str | None:
    """The message of the bound refusal that ``rule`` raises, or None when it accepts."""
    try:
        rule()
    except OperationFailed as error:
        return str(error)
    return None


def within_case(value: str, low: int, high: int, name: str) -> list[object]:
    return [value, low, high, name, refusal(lambda: bounds.within(int(value), low, high, name))]


def at_least_case(value: str, low: int, name: str) -> list[object]:
    return [value, low, name, refusal(lambda: bounds.at_least(int(value), low, name))]


def float_case(value: float) -> list[object]:
    return [bits(value), refusal(lambda: bounds.within(value, 0.0, 100.0, "resolution"))]


def bounds_fixture() -> Fixture:
    """Python's int() reading and the two bound rules over ints and floats."""
    values = ["0", "1", "5", "199", "200", "201", "64000", "64001", "-1", "-0", "9" * 25]
    ranges = [(1, 500, "limit"), (200, 64000, "budget"), (0, 200, "context")]
    floats = [0.0, -0.0, 1.0, 100.0, 100.00000000000001, -1e-300, 1e-300, math.inf, -math.inf]
    floats += [math.nan, 0.5, 1e16, 2.5e-5]
    return {
        "ints": [[text, python_int(text)] for text in INT_TEXTS],
        "deviations": [[text, python_int(text)] for text in DEVIATION_TEXTS],
        "within": [within_case(value, *bound) for value in values for bound in ranges],
        "at_least": [at_least_case(value, 1, name) for value in values for name in ["members"]],
        "floats": [float_case(value) for value in floats],
    }


TREE_LISTINGS = [
    [],
    ["a.py"],
    ["b.py", "a/x.py", "a/y/z.py", "a/y/w.py", "c/d/e/f/g.py"],
    ["src/app.py", "src/lib/core.py", "tests/test_app.py", "README.md", "zz", "Z", "_"],
    [
        "a\nforged.py",
        "dir\r/x.py",
        "\x00",
        "café/été.py",
        f"{LINE_SEPARATOR}/n.py",
    ],
    [f"pkg/m{index}.py" for index in range(12)] + ["pkg/sub/deep.py", "top.py"],
    ["x", "x/y.py"],
]
TREE_BOUNDS = [(4, 500), (1, 500), (2, 3), (3, 1), (20, 20000), (2, 7)]


def tree_case(paths: list[str], depth: int, max_entries: int) -> list[object]:
    """A walk lists its files sorted by path, so the case stores them that way."""
    listed = sorted(paths)
    files = [treewalk.RepoFile(path=path, size=0) for path in listed]
    rendered = treewalk.render_tree(files, depth=depth, max_entries=max_entries)
    return [listed, depth, max_entries, rendered]


SLICE_PATH = "lines.txt"
SLICE_TEXT = "".join(f"line {number}\n" for number in range(1, 101))
# A transposed range, a range from line 0, a bad range beside a good one, ranges past the end,
# and a good range.
SLICE_REQUESTS = [[(60, 30)], [(0, 5)], [(60, 30), (1, 2)], [(150, 160), (99, 120)], [(3, 5)]]
SLICE_CONTEXTS = [0, 20]


def slice_cases() -> list[object]:
    """The slice view's text of each request over a 100-line file in no language."""
    with TemporaryDirectory() as scratch:
        root = Path(scratch).resolve()
        (root / SLICE_PATH).write_text(SLICE_TEXT, encoding="utf-8")
        ctx = RepoContext(root=root, head_sha=None, tree_oid=None, dirty_count=None)
        views = ViewService(TreeSitterExtractor())
        return [
            [
                ranges,
                context,
                views.read_slice(ctx, SLICE_PATH, intervals=ranges, context=context).text,
            ]
            for ranges in SLICE_REQUESTS
            for context in SLICE_CONTEXTS
        ]


def views_fixture() -> Fixture:
    """The tree view's text over synthetic listings, and the slice view's text over one file."""
    return {
        "tree": [
            tree_case(paths, depth, entries)
            for paths in TREE_LISTINGS
            for depth, entries in TREE_BOUNDS
        ],
        "slice_file": [[SLICE_PATH, SLICE_TEXT]],
        "slice": slice_cases(),
    }


GOLDENS = ROOT / "tests" / "characterization" / "goldens"
TOKEN_CUTS = [None, 1, 7, 100, 1000]
EMOJI = chr(0x1F600)
TOKEN_TEXTS = [
    "",
    " ",
    "\n\n\n",
    "a  b   c\t\td",
    "don't won't I'll we're",
    "12345678901234567890",
    "def f(x):\r\n    return x  # note\r\n",
    f"café {EMOJI}{EMOJI} {LINE_SEPARATOR} {NO_BREAK_SPACE}x",
    "<|endoftext|> <|fim_prefix|>",
    "x" * 5000,
    " " * 300 + "y",
]


def cl100k_count(text: str) -> int:
    """The count of the cl100k_base tokens that tiktoken encodes ``text`` into."""
    return len(tiktoken.get_encoding("cl100k_base").encode_ordinary(text))


def golden_token_case(path: Path, cut: int | None) -> list[object]:
    text = path.read_text(encoding="utf-8")
    shown = text if cut is None else text[:cut]
    return [path.relative_to(GOLDENS).as_posix(), cut, cl100k_count(shown)]


def tokens_fixture() -> Fixture:
    """tiktoken's cl100k_base counts over every golden, prefixes of them, and edge texts."""
    goldens = sorted(path for path in GOLDENS.rglob("*") if path.is_file())
    return {
        "goldens": [golden_token_case(path, cut) for path in goldens for cut in TOKEN_CUTS],
        "texts": [[text, cl100k_count(text)] for text in TOKEN_TEXTS],
    }


def mcp_tools() -> dict[str, list[str]]:
    """The parameter names of each v1 MCP tool, in schema order."""
    app = server.build_server(mock.MagicMock(), "v1")
    tools = asyncio.run(app.list_tools())
    return {tool.name: list(tool.parameters.get("properties", {})) for tool in tools}


def doors_fixture() -> Fixture:
    """The v1 MCP read tools and their parameter names, for the cross-door test of OPS."""
    return {"tools": [[name, params] for name, params in sorted(mcp_tools().items())]}


READ_COMMANDS = [
    "map",
    "tree",
    "skeleton",
    "expand",
    "slice",
    "find-symbol",
    "refs",
    "history",
    "explain",
    "path",
    "cycles",
    "communities",
    "diagram",
    "health",
    "html",
    "resolve-locs",
    "warmup",
    "index",
    "guide",
]
HELP_WIDTHS = [None, "40", "200"]
# Each command line stops at its parse: a usage error, so no run reads a repository.
USAGE_ERRORS = [
    ["map", "--budget"],
    ["map", "--granularity", "bogus"],
    ["map", "--max-files", "x"],
    ["map", "--max-files", "1.5"],
    ["map", "--max", "x"],
    ["map", "--focus", "a", "extra"],
    ["map", "--max-files="],
    ["tree", "--depth", "abc"],
    ["tree", "--max", ""],
    ["tree", "deep"],
    ["skeleton"],
    ["skeleton", "--docstrings"],
    ["skeleton", "a.py", "--numbers", "b.py"],
    ["expand"],
    ["expand", "py:a.py::f", "--limit", "x"],
    ["slice", "a.py", "--symbol", "py:a.py::f"],
    ["slice", "--symbol", "py:a.py::f", "a.py"],
    ["slice", "--lines"],
    ["slice", "--context", "x"],
    ["slice", "a.py", "b.py"],
    ["find-symbol"],
    ["find-symbol", "x", "--kind", "bogus"],
    ["find-symbol", "x", "y"],
    ["find-symbol", "--kind=method"],
    ["refs"],
    ["refs", "a", "b"],
    ["history"],
    ["history", "x", "--budget", "x"],
    ["explain"],
    ["explain", "x", "--limit", "9" * 30 + "x"],
    ["path"],
    ["path", "a"],
    ["path", "a", "b", "c"],
    ["path", "a", "--include-unique"],
    ["path", "a", "b", "--max-visited", "--include-unique"],
    ["cycles", "--limit", "x"],
    ["cycles", "--limit", "1e3"],
    ["cycles", "--limit", "0x10"],
    ["cycles", "extra"],
    ["communities", "--resolution", "x"],
    ["communities", "--resolution", "1__0"],
    ["communities", "--resolution", "infx"],
    ["communities", "--resolution", "0x1p3"],
    ["communities", "--members", "x"],
    ["diagram", "--max-nodes", "x"],
    ["diagram", "--max", "3"],
    ["diagram", "--check"],
    ["diagram", "--focus"],
    ["health", "--limit", "1.0"],
    ["html", "--json"],
    ["html", "--max-edges", "x"],
    ["html", "--resolution", "nan_"],
    ["resolve-locs"],
    ["resolve-locs", "a.py"],
    ["resolve-locs", "--loc", "line: 3"],
    ["resolve-locs", "a.py", "--loc"],
    ["warmup", "--no-download", "--bogus"],
    ["index", "--no-cache"],
    ["index", "extra"],
    ["guide", "--section"],
    ["guide", "extra"],
    ["guide", "--repo", "x"],
]
FLOAT_TEXTS = [
    *["1", "1.5", " 2.5 ", "+.5", "5.", ".", "-0", "1e3", "1E-3", "1e", "e5", "1_000.5", "1__0"],
    *["_1", "1_", "1e1_0", "1_e1", "inf", "-Infinity", "INF", "nan", "NaN", "-nan", "infx"],
    *["0x1p3", "1.5\n", "", " ", "1 2", "+-1", "1.7976931348623157e309", "4.9e-324", "1e-400"],
    NO_BREAK_SPACE + "3" + NO_BREAK_SPACE,
]


def python_float(text: str) -> str | None:
    """The bits of float(text), or None when float() refuses it."""
    try:
        return bits(float(text))
    except ValueError:
        return None


def parser_fixture() -> Fixture:
    """Every read --help at three widths, and the usage error of each parser feature."""
    cases = [([command, "--help"], width) for command in READ_COMMANDS for width in HELP_WIDTHS]
    cases += [(argv, None) for argv in USAGE_ERRORS]
    with ThreadPoolExecutor(max_workers=8) as pool:
        rows: list[object] = list(pool.map(lambda case: cli_fixtures.parser_case(*case), cases))
    return {"runs": rows}


FIXTURE_REPOS = ROOT / "tests" / "characterization" / "fixtures"


def scenario(
    name: str, steps: list[list[str]], argv: list[str], cwd: str = "repo"
) -> dict[str, object]:
    return {"name": name, "steps": steps, "cwd": cwd, "argv": argv}


def copied(repo: str, place: str = "repo") -> list[list[str]]:
    """The steps that rebuild a characterization fixture repository as one commit."""
    source = FIXTURE_REPOS / repo
    files = sorted(path for path in source.rglob("*") if path.is_file())
    writes = [
        ["write", f"{place}/{path.relative_to(source).as_posix()}", path.read_text("utf-8")]
        for path in files
    ]
    return [["init", place], *writes, ["commit", place]]


KNOTS = [
    ["init", "repo"],
    ["write", "repo/pkg/__init__.py", ""],
    *[
        ["write", f"repo/pkg/{name}.py", f"from pkg import {other}\n"]
        for name, other in [("a", "b"), ("b", "a"), ("c", "d"), ("d", "e"), ("e", "c")]
    ],
    ["commit", "repo"],
]
PLAIN = [["write", "plain/a.py", "x = 1\n"]]
# repo_py with a file no grammar reads, a project config, and a file outside the repository.
VIEWED = [
    ["write", "outside.py", "x = 1\n"],
    *copied("repo_py")[:-1],
    ["write", "repo/README.md", "# repo\n\nnotes\n"],
    ["commit", "repo"],
]
DOCSTRINGS_ON = [*VIEWED, ["write", "repo/.agentless-mcp.json", '{"docstrings": true}\n']]
SKELETON = ["skeleton", "inventory.py"]
SLICE = ["slice", "inventory.py"]
LOCS = ["resolve-locs", "inventory.py"]
SYMBOL = ["slice", "--symbol"]
VIEW_SCENARIOS = [
    scenario("skeleton_one", VIEWED, SKELETON),
    scenario("skeleton_many_json", VIEWED, [*SKELETON, "pricing.py", "--json"]),
    scenario("skeleton_docstrings_numbers", VIEWED, [*SKELETON, "--docstrings", "--numbers"]),
    scenario("skeleton_config_docstrings", DOCSTRINGS_ON, SKELETON),
    scenario("skeleton_missing", VIEWED, [*SKELETON, "missing.py"]),
    scenario("skeleton_refused", VIEWED, [*SKELETON, "../outside.py"]),
    scenario("skeleton_no_grammar", VIEWED, [*SKELETON, "README.md", "--json"]),
    scenario("slice_lines", VIEWED, [*SLICE, "--lines", "3:5"]),
    scenario("slice_whole", VIEWED, ["slice", "pricing.py"]),
    scenario("slice_single_line", VIEWED, [*SLICE, "--lines", "50", "--context", "0"]),
    scenario(
        "slice_multi_json",
        VIEWED,
        [*SLICE, "--lines", "1:2", "--lines", "60:62", "--context", "1", "--json"],
    ),
    scenario("slice_beyond", VIEWED, [*SLICE, "--lines", "500:600"]),
    scenario("slice_partly_beyond", VIEWED, [*SLICE, "--lines", "2:3", "--lines", "900:901"]),
    scenario("slice_no_grammar", VIEWED, ["slice", "README.md"]),
    scenario("slice_bad_lines", VIEWED, [*SLICE, "--lines", "5:3"]),
    scenario("slice_no_file", VIEWED, ["slice"]),
    scenario("slice_missing_file", VIEWED, ["slice", "nope.py"]),
    scenario("slice_refused", VIEWED, ["slice", "../outside.py"]),
    scenario("slice_context_refused", VIEWED, [*SLICE, "--context", "201"]),
    scenario("slice_symbol_function", VIEWED, [*SYMBOL, "py:inventory.py::load_inventory"]),
    scenario("slice_symbol_method", VIEWED, [*SYMBOL, "py:inventory.py::Inventory.add"]),
    scenario("slice_symbol_class", VIEWED, [*SYMBOL, "py:pricing.py::Tier", "--json"]),
    scenario("slice_symbol_constant", VIEWED, [*SYMBOL, "py:pricing.py::CURRENCY"]),
    scenario("slice_symbol_missing", VIEWED, [*SYMBOL, "py:pricing.py::nothing"]),
    scenario("slice_symbol_bad_id", VIEWED, [*SYMBOL, "nothing"]),
    scenario("slice_symbol_with_lines", VIEWED, [*SYMBOL, "py:pricing.py::Tier", "--lines", "1"]),
    scenario("locs_text", VIEWED, [*LOCS, "--loc", "function: load_inventory", "--loc", "line: 3"]),
    scenario("locs_json", VIEWED, [*LOCS, "--loc", "class: Inventory", "--json"]),
    scenario("locs_unknown", VIEWED, [*LOCS, "--loc", "nonsense", "--context", "0"]),
    scenario("locs_missing_file", VIEWED, ["resolve-locs", "nope.py", "--loc", "line: 1"]),
]


MAPPED = copied("repo_py_tests")
NOTES = (
    "from inventory import load_inventory\n\n\n"
    "def restock(path):\n"
    "    # WHY: the warehouse feed lags a day, see ADR-12\n"
    "    inventory = load_inventory(path)\n"
    "    # NOTE: keeps\n    # two lines\n"
    "    return inventory\n"
)
NOTED = [*copied("repo_py")[:-1], ["write", "repo/notes.py", NOTES], ["commit", "repo"]]
MAP_CONFIG = '{"map_budget": 300, "max_files": 3}\n'
FILES_CONFIG = '{"granularity": "file", "stoplist": ["add"], "relation_weights": true}\n'
CONFIGURED = [*MAPPED[:-1], ["write", "repo/.agentless-mcp.json", MAP_CONFIG], ["commit", "repo"]]
FILES_ONLY = [*MAPPED[:-1], ["write", "repo/.agentless-mcp.json", FILES_CONFIG], ["commit", "repo"]]
NO_SYMBOLS = [["init", "repo"], ["write", "repo/README.md", "# repo\n"], ["commit", "repo"]]
MAP = ["map"]
FOCUS = [*MAP, "--focus"]
MAP_SCENARIOS = [
    scenario("map_text", copied("repo_py"), MAP),
    scenario("map_json", copied("repo_ts"), [*MAP, "--json"]),
    scenario("map_go", copied("repo_go"), MAP),
    scenario("map_companions", MAPPED, MAP),
    scenario("map_companions_json", MAPPED, [*MAP, "--json"]),
    scenario("map_focus_path", MAPPED, [*FOCUS, "pricing.py", "--max-files", "3"]),
    scenario("map_focus_names_json", MAPPED, [*FOCUS, "inventory", *FOCUS[1:], "Inventory.add"]),
    scenario(
        "map_focus_spellings",
        MAPPED,
        [
            *FOCUS,
            'File "/srv/app/pricing.py", line 3, in quote',
            "--focus",
            "https://example.com/o/r/blob/main/tests/test_cli.py#L4-L9",
            "--focus",
            "shipping.py:12",
        ],
    ),
    scenario("map_focus_unresolved", MAPPED, [*FOCUS, "zz_nothing", *FOCUS[1:], "  "]),
    scenario("map_focus_unreached", MAPPED, [*FOCUS, "tests/test_cli.py", "--max-files", "6"]),
    scenario("map_file_granularity", MAPPED, [*MAP, "--granularity", "file"]),
    scenario("map_file_json", MAPPED, [*FOCUS, "cli.py", "--granularity", "file", "--json"]),
    scenario("map_budget_small", MAPPED, [*MAP, "--budget", "200"]),
    scenario("map_budget_auto_json", copied("repo_py"), [*MAP, "--budget", "auto", "--json"]),
    scenario("map_budget_underscore", MAPPED, [*MAP, "--budget", "1_000", "--max-files", "4"]),
    scenario("map_budget_zero", MAPPED, [*MAP, "--budget", "0"]),
    scenario("map_budget_word", MAPPED, [*MAP, "--budget", "lots"]),
    scenario("map_budget_below_range", MAPPED, [*MAP, "--budget", "199"]),
    scenario("map_max_files_above_range", MAPPED, [*MAP, "--max-files", "201"]),
    scenario("map_config", CONFIGURED, MAP),
    scenario("map_config_auto_wins", CONFIGURED, [*MAP, "--budget", "auto", "--max-files", "2"]),
    scenario("map_config_files_json", FILES_ONLY, [*MAP, "--json"]),
    scenario("map_rationale", NOTED, [*FOCUS, "notes.py"]),
    scenario("map_rationale_json", NOTED, [*FOCUS, "notes.py", "--json"]),
    scenario("map_no_cache", copied("repo_py"), [*MAP, "--no-cache", "--max-files", "2"]),
    scenario("map_no_symbols", NO_SYMBOLS, MAP),
    scenario("map_outside_git", PLAIN, MAP, "plain"),
]


READ_SCENARIOS = [
    scenario("tree_text", copied("repo_py"), ["tree"]),
    scenario("tree_json", copied("repo_ts"), ["tree", "--json"]),
    scenario("tree_cut", copied("repo_py_tests"), ["tree", "--depth", "1", "--max-entries", "3"]),
    scenario("tree_repo_flag", copied("repo_go"), ["tree", "--repo", "<SANDBOX>/repo"], "."),
    scenario("tree_depth_refused", copied("repo_py"), ["tree", "--depth", "0"]),
    scenario("tree_outside_git", PLAIN, ["tree"], "plain"),
    scenario("tree_plain_repo_flag", PLAIN, ["tree", "--repo", "."], "plain"),
    scenario("cycles_text", copied("repo_nested"), ["cycles"]),
    scenario("cycles_json", copied("repo_nested"), ["cycles", "--json"]),
    scenario("cycles_none", copied("repo_py"), ["cycles"]),
    scenario("cycles_cut", KNOTS, ["cycles", "--limit", "1"]),
    scenario("cycles_cut_json", KNOTS, ["cycles", "--limit", "1", "--json"]),
    scenario("cycles_limit_refused", KNOTS, ["cycles", "--limit", "501"]),
    scenario("cycles_no_cache", KNOTS, ["cycles", "--no-cache"]),
    scenario("guide_whole", PLAIN, ["guide"], "plain"),
    scenario("guide_section", PLAIN, ["guide", "--section", "refs"], "plain"),
    scenario("guide_unknown", PLAIN, ["guide", "--section", "nope"], "plain"),
    scenario("warmup_all", PLAIN, ["warmup"], "plain"),
    scenario("warmup_named", PLAIN, ["warmup", "rust", "python", "rust"], "plain"),
    scenario("warmup_no_download", PLAIN, ["warmup", "--no-download", "yaml"], "plain"),
    *VIEW_SCENARIOS,
]


def scenario_row(case: dict[str, object]) -> dict[str, object]:
    """One scenario's answer, with the scratch grammar cache as a placeholder."""
    row = cli_fixtures.scenario_case(case)
    grammars = os.environ[GRAMMAR_OVERRIDE]
    for stream in ("out", "err"):
        row[stream] = str(row[stream]).replace(grammars, "<GRAMMAR_CACHE>")
    return row


def runs_of(scenarios: list[dict[str, object]]) -> Fixture:
    with ThreadPoolExecutor(max_workers=8) as pool:
        rows: list[object] = list(pool.map(scenario_row, scenarios))
    return {"runs": rows}


def scenarios_fixture() -> Fixture:
    """Read subcommands other than map run end to end over sandbox repositories."""
    return runs_of(READ_SCENARIOS)


def map_scenarios_fixture() -> Fixture:
    """The map run end to end over sandbox repositories, in its own file to keep each file small."""
    return runs_of(MAP_SCENARIOS)


FOCUS_ENTRIES = {
    "repo_py_tests": [
        "inventory.py",
        "tests/test_cli.py",
        "test_cli.py",
        "inventory",
        "tests/test_cli",
        "test_pricing",
        "Inventory",
        "Inventory.add",
        "py:inventory.py::Inventory.add",
        "x::Inventory",
        "::",
        "Inventory.",
        "a..b",
        ".py",
        "__init__",
        "Item.__init__",
        "json",
        "json.dumps",
        "billing",
        "account",
        "command",
        "sku",
        "str",
        "pricing.txt",
        "nope/pricing.txt",
        'File "/srv/app/pricing.py", line 3, in quote',
        "https://github.com/o/r/blob/main/tests/test_cli.py#L4-L9",
        "shipping.py:12",
        "C:\\work\\repo\\cli.py",
        "/home/x/repo/orders.py",
        "orders.py#L3",
        "zz_nothing",
    ],
    "repo_go": [
        "Inventory.Add",
        "Add",
        "pricing",
        "MaxItems",
        "go:inventory.go::Inventory.Add",
        "Sprintf",
        "err",
        "fmt.Sprintf",
    ],
    "repo_ts": ["constructor", "Inventory.constructor", "types", "add", "length", "Record"],
}
SPELLING_ENTRIES = [
    '\tFile "x/y.py", line 9',
    'File "", line 3',
    'File "a.py", line',
    "ftp+x.y-z://host/a/b.py",
    "1http://x/a.py",
    "a.py:12-40\n",
    "a.py#L3-L",
    "a.py:" + ARABIC_TWELVE,
    "a.py:12:",
    "x\\y\\z.py",
    "/".join(["p"] * 40) + "/z.py",
    "./a/./b.py",
    "//net/share/a.py",
    ".",
    "",
    "C:\\x.py#L9",
]


def focus_fixture() -> Fixture:
    """The map's focus resolution: each entry's files and definitions, and the seed weights."""
    extractor = TreeSitterExtractor()
    resolutions: list[object] = []
    seedings: list[object] = []
    for repo, entries in FOCUS_ENTRIES.items():
        scan = refs.scan_repo((FIXTURE_REPOS / repo).resolve(), extractor)
        index = refs.build_ref_index(scan)
        known = {facts.path for facts in scan.files}
        for entry in entries:
            paths, matched = map_service._focus_resolution(entry, known, index)
            ids = [symbol_stable_id(definition.symbol) for definition in matched]
            resolutions.append([repo, entry, paths, ids])
        asked = (*entries, "  ", "zz_nothing")
        seeding = map_service.seed_weights(asked, scan, index)
        weights = [[path, bits(weight)] for path, weight in sorted(seeding.weights.items())]
        ids = [symbol_stable_id(definition.symbol) for definition in seeding.definitions]
        seedings.append([repo, list(asked), weights, list(seeding.unresolved), ids])
    spellings: list[object] = [
        [entry, map_service._normalized_spellings(entry)] for entry in SPELLING_ENTRIES
    ]
    return {"resolutions": resolutions, "seedings": seedings, "spellings": spellings}


# The companions' weights are sums over a set, whose order follows the hash seed.
HASH_SEEDS = ["0", "12345"]
COMPANION_CASES = [
    ("repo_py_tests", [], 10),
    ("repo_py_tests", [], 3),
    ("repo_py_tests", ["pricing.py"], 1),
    ("repo_py_tests", ["cli.py", "inventory"], 2),
    ("repo_py_tests", ["Inventory.add"], 1),
    ("repo_py_tests", ["audit.py"], 1),
    ("repo_py_tests", ["warehouse.py"], 1),
    ("repo_py_tests", ["reports.py", "shipping.py", "orders.py"], 3),
    ("repo_py", [], 10),
    ("repo_ts", ["types.ts"], 1),
]
COMPANION_CODE = """
import json, struct, sys
from pathlib import Path
from agentless_mcp.application.map_service import MapRequest, MapService
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.tokens import Chars4Counter
root, focus = Path(sys.argv[1]).resolve(), tuple(json.loads(sys.argv[2]))
ctx = RepoContext(root=root, head_sha="0000000f", tree_oid="1111111f", dirty_count=0, note="")
maps = MapService(TreeSitterExtractor(), Chars4Counter())
request = MapRequest(focus=focus, max_files=int(sys.argv[3]))
listing = maps.build(ctx, request).test_companions
def bits(x):
    return "0x%016x" % struct.unpack("<Q", struct.pack("<d", x))[0]
rows = [[r.path, r.start, r.end, list(r.covers), r.depth, bits(r.weight)] for r in listing.rows]
answer = {"total": listing.total, "limit": listing.limit, "exhausted": listing.exhausted}
print(json.dumps({**answer, "rows": rows}))
"""


def companions_answer(repo: str, focus: list[str], max_files: int, seed: str) -> object:
    where = str(FIXTURE_REPOS / repo)
    command = [sys.executable, "-c", COMPANION_CODE, where, json.dumps(focus), str(max_files)]
    env = {**os.environ, "PYTHONHASHSEED": seed}
    done = subprocess.run(
        command, cwd=ROOT, env=env, capture_output=True, timeout=120, check=True, text=True
    )
    return json.loads(done.stdout)


def companions_fixture() -> Fixture:
    """The test companions of a map under two hash seeds; the second is null when they agree."""
    rows: list[object] = []
    for repo, focus, max_files in COMPANION_CASES:
        first, second = (companions_answer(repo, focus, max_files, seed) for seed in HASH_SEEDS)
        rows.append([repo, focus, max_files, first, None if second == first else second])
    return {"cases": rows}


def numbers_fixture() -> Fixture:
    """Python's float() reading of the texts a --resolution value can hold."""
    return {
        "floats": [[text, python_float(text)] for text in FLOAT_TEXTS],
        "deviations": [[text, python_float(text)] for text in DEVIATION_TEXTS],
    }


APP_BUILDERS: dict[str, Callable[[], Fixture]] = {
    "bounds": bounds_fixture,
    "companions": companions_fixture,
    "doors": doors_fixture,
    "focus": focus_fixture,
    "views": views_fixture,
    "tokens": tokens_fixture,
}
CLI_BUILDERS: dict[str, Callable[[], Fixture]] = {
    "parser": parser_fixture,
    "numbers": numbers_fixture,
    "scenarios": scenarios_fixture,
    "map_scenarios": map_scenarios_fixture,
}


def build() -> dict[Path, bytes]:
    app = {APP / f"{name}.json": render(builder()) for name, builder in APP_BUILDERS.items()}
    cli = {CLI / f"{name}.json": render(builder()) for name, builder in CLI_BUILDERS.items()}
    return {**app, **cli}


def main() -> int:
    refuse_unready_environment()
    return write_or_check_files(__doc__.splitlines()[0], APP, build)


if __name__ == "__main__":
    sys.exit(main())
