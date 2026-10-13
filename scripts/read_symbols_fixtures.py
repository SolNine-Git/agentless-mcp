"""Write the symbol-view fixtures of the app and cli crates: what the Python build answers.

Development-time only. Run it from the repository root, with the grammar libraries copied to a
scratch directory and the caches inside a scratch directory, so that no run reads or writes the
user's caches:

    XDG_CACHE_HOME=<scratch>/cache TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy>/v1.14.3/libs \
        TIKTOKEN_CACHE_DIR=<scratch copy of the cl100k_base file> \
        .venv/bin/python scripts/read_symbols_fixtures.py

With --check, the script compares its output with the committed fixtures and writes nothing.

The files live in the read_symbols/ subdirectory of each crate's fixtures: find-symbol, expand,
refs and history, run end to end over sandbox repositories, and the budget split they share.
"""

import random
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from fixture_files import Fixture, refuse_real_caches, render, write_or_check_files
from read_fixtures import copied, scenario, scenario_row

from agentless_mcp.util.budget import allocate

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "crates" / "app" / "fixtures" / "read_symbols"
CLI = ROOT / "crates" / "cli" / "fixtures" / "read_symbols"

ALLOCATE_COSTS = [
    [],
    [0],
    [5],
    [5, 5],
    [10, 1, 1],
    [100, 3, 50, 7],
    [0, 0, 9],
    [1] * 7,
    [30, 30, 30],
    [12000],
    [13000, 13000],
    list(range(1, 21)),
    [4000, 4000, 4000, 100],
    [3999, 4001, 4002, 1, 2],
]
ALLOCATE_BUDGETS = [0, 1, 10, 31, 100, 1000, 12000, 64000]


def seeded_costs() -> list[list[int]]:
    """Cost lists drawn from a fixed seed, so the fixture never moves between runs."""
    draw = random.Random(20261012)
    return [[draw.randrange(0, 3000) for _ in range(draw.randrange(1, 30))] for _ in range(12)]


def budget_fixture() -> Fixture:
    """The max-min fair split: the indices it cuts and the share each cut item gets."""
    rows: list[object] = []
    for costs in [*ALLOCATE_COSTS, *seeded_costs()]:
        for budget in ALLOCATE_BUDGETS:
            cut, share = allocate(costs, budget)
            rows.append([costs, budget, sorted(cut), share])
    return {"allocate": rows}


SHAPES = '''"""Shapes and their makers."""


class Shape:
    """A shape with no size."""

    def area(self):
        return 0

    def perimeter(self):
        return 0


class Square(Shape):
    # WHY: the side is set after construction.
    def area(self):
        return self.side ** 2


def make_shape(kind):
    return Square() if kind == "square" else Shape()


def make_square():
    return make_shape("square")


SHAPE_LIMIT = 10
'''
# Six lines that spell the query inside a region that did not parse, and a seventh in another file.
BROKEN = """def fine():
    return 1

def (lost_widget:
    lost_widget(1)
    print(lost_widget)
    lost_widget.x
    [lost_widget]
    {lost_widget}
    pass
"""
BROKEN_TOO = "def (Lost_Widget:\n    pass\n"
UTIL_TS = """export function makeLabel(name: string): string {
  return name.toUpperCase();
}

export class Labeler {
  label(name: string): string {
    return makeLabel(name);
  }
}
"""
# A key that Unicode 16 cases and Unicode 15.1 does not: find lowercases it as Python 3.13 does.
DATA_JSON = '{"Ɤ": 1, "ɤ": 2, "Area": 3}\n'
SYMBOLS = [
    ["init", "repo"],
    ["write", "repo/shapes.py", SHAPES],
    ["write", "repo/broken.py", BROKEN],
    ["write", "repo/more/broken.py", BROKEN_TOO],
    ["write", "repo/ts/util.ts", UTIL_TS],
    ["write", "repo/data.json", DATA_JSON],
    ["write", "repo/README.md", "# shapes\n"],
    ["write", "repo/pkg.py/notes.txt", "not code\n"],
    ["commit", "repo"],
]


def numbered_functions(count: int, lines: int, width: int) -> str:
    """A module of ``count`` functions, each ``lines`` lines of ``width`` characters long."""
    blocks = []
    for index in range(count):
        body = [f"    value_{row} = {'x' * (width - 16)!r}" for row in range(lines - 2)]
        blocks.append("\n".join([f"def block_{index}():", *body, "    return None"]))
    return "\n\n\n".join(blocks) + "\n"


BIG = [
    ["init", "repo"],
    ["write", "repo/big.py", numbered_functions(4, 300, 50)],
    ["write", "repo/small.py", "def tiny():\n    return 1\n"],
    ["commit", "repo"],
]
MANY = [
    ["init", "repo"],
    ["write", "repo/many.py", numbered_functions(45, 2, 20)],
    ["commit", "repo"],
]
BIG_IDS = [f"py:big.py::block_{index}" for index in range(4)]
MANY_IDS = [f"py:many.py::block_{index}" for index in range(45)]
BOGUS_IDS = [f"py:shapes.py::gone_{index}" for index in range(25)]
MISSES = [
    "py:shapes.py::make_shape",
    "py:shapes.py::gone",
    "nonsense",
    "rs:shapes.py::make_shape",
    "py:../outside.py::f",
    "py:README.md::x",
    "py:missing.py::f",
    "py:pkg.py::f",
    "py:shapes.py::make_shape::rationale@3",
]

FIND_SCENARIOS = [
    scenario("find_text", copied("repo_py"), ["find-symbol", "format"]),
    scenario("find_json", copied("repo_ts"), ["find-symbol", "report", "--json"]),
    scenario("find_kind", SYMBOLS, ["find-symbol", "area", "--kind", "method"]),
    scenario("find_kind_miss", SYMBOLS, ["find-symbol", "area", "--kind", "class"]),
    scenario("find_kind_miss_json", SYMBOLS, ["find-symbol", "area", "--kind", "enum", "--json"]),
    scenario("find_miss", SYMBOLS, ["find-symbol", "zzz_nothing"]),
    scenario("find_qualified", SYMBOLS, ["find-symbol", "Square.area"]),
    scenario("find_rank", SYMBOLS, ["find-symbol", "make"]),
    scenario("find_case", SYMBOLS, ["find-symbol", "SHAPE"]),
    scenario("find_ts", SYMBOLS, ["find-symbol", "label"]),
    scenario("find_cut", copied("repo_py"), ["find-symbol", "", "--limit", "3"]),
    scenario("find_cut_json", copied("repo_py"), ["find-symbol", "", "--limit", "3", "--json"]),
    scenario("find_newer_capital", SYMBOLS, ["find-symbol", "ɤ"]),
    scenario("find_unparsed", SYMBOLS, ["find-symbol", "lost_widget"]),
    scenario("find_unparsed_json", SYMBOLS, ["find-symbol", "LOST_widget", "--json"]),
    scenario("find_limit_refused", SYMBOLS, ["find-symbol", "x", "--limit", "501"]),
]
EXPAND_SCENARIOS = [
    scenario(
        "expand_text",
        copied("repo_py"),
        ["expand", "py:reports.py::reorder_report", "py:pricing.py::format_money"],
    ),
    scenario(
        "expand_json",
        copied("repo_py"),
        ["expand", "py:reports.py::reorder_report", "py:pricing.py::format_money", "--json"],
    ),
    scenario("expand_method", SYMBOLS, ["expand", "py:shapes.py::Square.area"]),
    scenario("expand_ts", SYMBOLS, ["expand", "ts:ts/util.ts::Labeler.label"]),
    scenario("expand_misses", SYMBOLS, ["expand", *MISSES]),
    scenario("expand_misses_json", SYMBOLS, ["expand", *MISSES, "--json"]),
    scenario(
        "expand_repeated",
        SYMBOLS,
        ["expand", *["py:shapes.py::make_shape"] * 3, "py:shapes.py::Shape", "--limit", "1"],
    ),
    scenario("expand_limit_refused", SYMBOLS, ["expand", "py:shapes.py::Shape", "--limit", "0"]),
    scenario("expand_no_room", MANY, ["expand", *MANY_IDS, "--limit", "50"]),
    scenario("expand_omitted", SYMBOLS, ["expand", *BOGUS_IDS, "--limit", "30"]),
    scenario("expand_shortened", BIG, ["expand", *BIG_IDS, "py:small.py::tiny"]),
    scenario("expand_shortened_json", BIG, ["expand", *BIG_IDS, "py:small.py::tiny", "--json"]),
]
ALIAS_A = """def helper(value):
    return value + 1


def other():
    return helper(2)
"""
ALIAS_B = """from a import helper as assist


def run():
    return assist(1)


def again():
    assist(2)
    return assist(3)
"""
ALIAS_C = """import a


def go():
    return a.helper(4)
"""
ALIAS_D = """import { helper } from "./e";

export function useIt(): number {
  return helper(1);
}
"""
ALIAS_E = "export function helper(x: number): number {\n  return x;\n}\n"
ALIAS = [
    ["init", "repo"],
    *[
        ["write", f"repo/{name}", text]
        for name, text in [
            ("a.py", ALIAS_A),
            ("b.py", ALIAS_B),
            ("c.py", ALIAS_C),
            ("d.ts", ALIAS_D),
            ("e.ts", ALIAS_E),
        ]
    ],
    ["commit", "repo"],
]
UTIL = """def format_name(name):
    return name.title()


def clean(text):
    return text.strip()


def log(message):
    return message
"""
FIXTURE_HELPER = "def fixture_name():\n    return 'x'\n"


def handler(index: int) -> str:
    """One of seven callers that share format_name, clean, log and a test helper."""
    imports = "from util import clean, format_name, log\nfrom tests.helpers import fixture_name\n"
    body = "    log(value)\n    fixture_name()\n    return clean(format_name(value))\n"
    return f"{imports}\n\ndef handle_{index}(value):\n{body}"


SHARED = [
    ["init", "repo"],
    ["write", "repo/util.py", UTIL],
    ["write", "repo/tests/helpers.py", FIXTURE_HELPER],
    *[["write", f"repo/app{index}.py", handler(index)] for index in range(7)],
    ["commit", "repo"],
]
SHARED_ARGV = ["refs", "format_name", "--shared-callers"]
REFS_SCENARIOS = [
    scenario("refs_text", copied("repo_py"), ["refs", "format_money"]),
    scenario("refs_json", copied("repo_py"), ["refs", "format_money", "--json"]),
    scenario("refs_nested", copied("repo_nested"), ["refs", "format_amount"]),
    scenario("refs_ts", copied("repo_ts"), ["refs", "formatMoney", "--json"]),
    scenario("refs_alias", ALIAS, ["refs", "py:a.py::helper"]),
    scenario("refs_alias_json", ALIAS, ["refs", "py:a.py::helper", "--json"]),
    scenario("refs_bare_name", ALIAS, ["refs", "helper"]),
    scenario("refs_cut", ALIAS, ["refs", "helper", "--limit", "2"]),
    scenario("refs_cut_json", ALIAS, ["refs", "helper", "--limit", "2", "--json"]),
    scenario("refs_none", copied("repo_py"), ["refs", "summarise"]),
    scenario("refs_moved", copied("repo_py"), ["refs", "py:gone.py::format_money"]),
    scenario("refs_moved_none", copied("repo_py"), ["refs", "py:reports.py::nothing_here"]),
    scenario("refs_dotted_miss", copied("repo_py"), ["refs", "elsewhere.format_money"]),
    scenario("refs_shared", SHARED, SHARED_ARGV),
    scenario("refs_shared_json", SHARED, [*SHARED_ARGV, "--json"]),
    scenario("refs_shared_cut", SHARED, [*SHARED_ARGV, "--limit", "1"]),
    scenario("refs_shared_none", copied("repo_py"), ["refs", "summarise", "--shared-callers"]),
    scenario(
        "refs_shared_tests_repo",
        copied("repo_py_tests"),
        ["refs", "py:pricing.py::PriceBook.quote", "--shared-callers"],
    ),
    scenario("refs_limit_refused", copied("repo_py"), ["refs", "format_money", "--limit", "0"]),
]
CALC_V1 = "def add(a, b):\n    return a + b\n"
CALC_V2 = """def add(a, b):
    if a is None or b is None:
        raise ValueError("operand")
    return a + b
"""
CALC_V3 = CALC_V2.replace("return a + b", "return round(a + b, 2)")
CALC_V4 = CALC_V3 + "\n\ndef sub(a, b):\n    return a - b\n"
CALC_V5 = CALC_V4.replace('"operand"', '"no operand"')
CALC_V6 = CALC_V5.replace("round(a + b, 2)", "round(a + b, 3)")
CALC_V7 = CALC_V6.replace("round(a + b, 3)", "round(a + b, 4)")
LONG_BODY = "\n".join(f"Line {index} of a long reason, kept as written." for index in range(14))
HISTORY_COMMITS = [
    (CALC_V1, "Add the calculator\n\nThe first version adds two numbers.\nIt has no checks."),
    (CALC_V2, f"Check the operands\n\n\n  Refuse a None operand.\n\n{LONG_BODY}\n\n"),
    (CALC_V3, "Round the result"),
    (CALC_V4, "Add subtraction\n\nsub() sits below add()."),
    (CALC_V5, "Name the missing operand\n\nWHY: the old text\nsaid nothing."),
    (CALC_V6, "Round to three places"),
    (CALC_V7, "Round to four places"),
]
HISTORY = [
    ["init", "repo"],
    *[
        step
        for text, message in HISTORY_COMMITS
        for step in (["write", "repo/calc.py", text], ["commit", "repo", message])
    ],
    ["write", "repo/notes.txt", "unrelated\n"],
    ["commit", "repo", "Unrelated change"],
]
# Two lines above add() and a changed signature, uncommitted: git reads the committed span.
DIRTY_CALC = "import math\n\n" + CALC_V7.replace("(a, b)", "(a, b, c=0)", 1)
DIRTY = [*HISTORY, ["write", "repo/calc.py", DIRTY_CALC]]
ADDED = [*HISTORY, ["write", "repo/calc.py", CALC_V7 + "\n\ndef mul(a, b):\n    return a * b\n"]]
UNTRACKED = [*HISTORY, ["write", "repo/new.py", "def fresh():\n    return 1\n"]]
PLAIN_CALC = [["write", "plain/calc.py", CALC_V1]]
ADD = ["history", "py:calc.py::add"]
HISTORY_SCENARIOS = [
    scenario("history_text", HISTORY, ADD),
    scenario("history_json", HISTORY, [*ADD, "--json"]),
    scenario("history_limit", HISTORY, [*ADD, "--limit", "2"]),
    scenario("history_budget", HISTORY, [*ADD, "--budget", "200"]),
    scenario("history_budget_json", HISTORY, [*ADD, "--budget", "200", "--json"]),
    scenario("history_one_commit", HISTORY, ["history", "py:calc.py::sub"]),
    scenario("history_dirty", DIRTY, ADD),
    scenario("history_dirty_json", DIRTY, [*ADD, "--json"]),
    scenario("history_not_in_head", ADDED, ["history", "py:calc.py::mul"]),
    scenario("history_untracked", UNTRACKED, ["history", "py:new.py::fresh"]),
    scenario("history_unresolved", HISTORY, ["history", "py:calc.py::nope"]),
    scenario("history_malformed_id", HISTORY, ["history", "nonsense"]),
    scenario("history_outside", HISTORY, ["history", "py:../calc.py::add"]),
    scenario("history_no_git", PLAIN_CALC, [*ADD, "--repo", "."], "plain"),
    scenario("history_budget_refused", HISTORY, [*ADD, "--budget", "199"]),
    scenario("history_limit_refused", HISTORY, [*ADD, "--limit", "501"]),
]
BODY = ["map", "--granularity", "body"]
BODY_SCENARIOS = [
    scenario("map_body_text", copied("repo_py"), BODY),
    scenario("map_body_json", copied("repo_py"), [*BODY, "--json"]),
    scenario(
        "map_body_focus",
        copied("repo_py_tests"),
        [*BODY, "--focus", "py:pricing.py::PriceBook.quote", "--focus", "reports.py"],
    ),
    scenario("map_body_budget", copied("repo_nested"), [*BODY, "--budget", "600"]),
    scenario("map_body_cut", BIG, [*BODY, "--budget", "1000"]),
    scenario("map_body_cut_json", BIG, [*BODY, "--budget", "1000", "--json"]),
    scenario("map_body_budget_refused", copied("repo_py"), [*BODY, "--budget", "100"]),
]
SCENARIOS = [
    *FIND_SCENARIOS,
    *EXPAND_SCENARIOS,
    *REFS_SCENARIOS,
    *HISTORY_SCENARIOS,
    *BODY_SCENARIOS,
]


def scenario_rows() -> list[object]:
    """Each scenario's answer, run eight at a time."""
    with ThreadPoolExecutor(max_workers=8) as pool:
        return list(pool.map(scenario_row, SCENARIOS))


def build() -> dict[Path, bytes]:
    files = {
        APP / "budget.json": budget_fixture(),
        CLI / "scenarios.json": {"runs": scenario_rows()},
    }
    return {path: render(sections) for path, sections in files.items()}


def main() -> int:
    refuse_real_caches()
    CLI.mkdir(parents=True, exist_ok=True)
    return write_or_check_files(__doc__.splitlines()[0], APP, build)


if __name__ == "__main__":
    sys.exit(main())
