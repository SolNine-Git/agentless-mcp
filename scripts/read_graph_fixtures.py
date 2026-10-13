"""Write the graph-view fixtures of the app, render and cli crates: what the Python build answers.

Development-time only. Run it from the repository root, with the grammar libraries copied to a
scratch directory and the caches inside a scratch directory, so that no run reads or writes the
user's caches:

    XDG_CACHE_HOME=<scratch>/cache TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy>/v1.14.3/libs \
        TIKTOKEN_CACHE_DIR=<scratch copy of the cl100k_base file> \
        .venv/bin/python scripts/read_graph_fixtures.py

With --check, the script compares its output with the committed fixtures and writes nothing.

The files live in the read_graph/ subdirectory of each crate's fixtures: the diagram labels, the
fence rule, the HTML export documents, and explain, path, communities, diagram, html, health and
index run end to end over sandbox repositories.
"""

import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from fixture_files import Fixture, refuse_real_caches, render, write_or_check_files
from read_fixtures import copied, scenario, scenario_row

from agentless_mcp.application import render as views
from agentless_mcp.application.graph_service import GraphService
from agentless_mcp.application.repo_context import RepoContext
from agentless_mcp.core import mermaid
from agentless_mcp.core.extractor import TreeSitterExtractor

ROOT = Path(__file__).resolve().parents[1]
APP = ROOT / "crates" / "app" / "fixtures" / "read_graph"
RENDER = ROOT / "crates" / "render" / "fixtures" / "read_graph"
CLI = ROOT / "crates" / "cli" / "fixtures" / "read_graph"
FIXTURE_REPOS = ROOT / "tests" / "characterization" / "fixtures"
GOLDENS = ROOT / "tests" / "characterization" / "goldens" / "graph"

LABEL_TEXTS = [
    "src/app.py",
    'a"];click x',
    "café/  x__y",
    "   ",
    "",
    chr(0x1F600),
    "a\tb\nc",
    "__init__.py",
    " _ lead and trail _ ",
    "d/" * 40,
    "x" * 60,
    "x" * 61,
    "end",
    "repository root",
    "-.-> n0",
    "%% comment",
]
DISTINCT_CASES = [
    [],
    ["a"],
    ["a", "b", "a", "a"],
    ["a 2", "a", "a"],
    ["unnamed", "unnamed"],
]


def labels_fixture() -> Fixture:
    """The diagram's label allowlist and the ordinal it gives a repeated label."""
    return {
        "labels": [[text, mermaid.safe_label(text)] for text in LABEL_TEXTS],
        "distinct": [[case, mermaid._distinct(case)] for case in DISTINCT_CASES],
    }


FENCE_TEXTS = [
    "flowchart LR\n",
    "```mermaid\nflowchart LR\n```\n",
    "\n\n```mermaid\nflowchart LR\n    n0\n```\n\n\n",
    "```mermaid\nflowchart LR\n",
    "```\n```\n",
    "```\n",
    "```mermaid\n```",
    "  ```mermaid\nx\n```\n",
    "```mermaid\nx\n  ```  \n",
    "```mermaid\nx\n```\ntrailing\n",
    "",
    "\n \n",
    "```mermaid\r\nx\r\n```\r\n",
]


def fences_fixture() -> Fixture:
    """The fence a committed diagram may travel in, and the text inside it."""
    return {"fences": [[text, views.strip_fence(text)] for text in FENCE_TEXTS]}


HTML_REPOS = ["repo_py", "repo_ts", "repo_nested", "repo_py_tests"]
HTML_BOUNDS = [(200, 600), (2, 1), (1, 0)]


def pinned(repo: str) -> RepoContext:
    """The golden harness's context: pinned git state, no cache, the repository's config."""
    return RepoContext(
        root=(FIXTURE_REPOS / repo).resolve(),
        head_sha="0000000f",
        tree_oid="1111111f",
        dirty_count=0,
        note="",
    )


def html_fixture() -> Fixture:
    """The HTML export of each fixture repository under three pairs of bounds."""
    graphs = GraphService(TreeSitterExtractor())
    rows: list[object] = []
    for repo in HTML_REPOS:
        for nodes, edges in HTML_BOUNDS:
            exported = graphs.html(pinned(repo), max_nodes=nodes, max_edges=edges)
            rows.append([repo, nodes, edges, exported.text])
    return {"documents": rows}


NESTED = copied("repo_nested")
PY = copied("repo_py")
TS = copied("repo_ts")
TESTS = copied("repo_py_tests")
# A unique, an ambiguous, a same-file and an imported reference, with no import behind the first
# three, so the diagram draws reference edges dashed.
REFS_FILES = {
    "a.py": "def helper():\n    return 1\n\n\ndef twin():\n    return 2\n",
    "b.py": "def twin():\n    return 3\n\n\ndef caller():\n    return helper() + twin()\n",
    "c.py": "def use():\n    return twin()\n",
    "d.py": "from a import helper\n\n\ndef again():\n    return helper()\n",
}
REFS = [
    ["init", "repo"],
    *[["write", f"repo/{name}", text] for name, text in REFS_FILES.items()],
    ["commit", "repo"],
]
NO_FILES = [["init", "repo"], ["write", "repo/README.md", "# repo\n"], ["commit", "repo"]]


def committed(text: str) -> list[list[str]]:
    """The nested repository with `text` committed as docs/graph.md."""
    return [*NESTED[:-1], ["write", "repo/docs/graph.md", text], ["commit", "repo"]]


GROUPED = (GOLDENS / "repo_nested.diagram.mmd").read_text(encoding="utf-8")
FENCED = f"\n```mermaid\n{GROUPED}```\n\n"
HEADED = f"# Modules\n\n```mermaid\n{GROUPED}```\n"
DRIFTED = GROUPED.replace("n0 --> n1", "n0 --> n2")
CHECK = ["diagram", "--communities", "--check", "docs/graph.md"]

EXPLAIN_SCENARIOS = [
    scenario("explain_text", NESTED, ["explain", "format_amount"]),
    scenario("explain_json", PY, ["explain", "reorder_report", "--json"]),
    scenario("explain_ts", TS, ["explain", "formatMoney"]),
    scenario("explain_stable_id", NESTED, ["explain", "py:billing/money.py::format_amount"]),
    scenario("explain_miss", PY, ["explain", "zz_nothing"]),
    scenario("explain_miss_json", PY, ["explain", "zz_nothing", "--json"]),
    scenario("explain_tie", REFS, ["explain", "twin"]),
    scenario("explain_tiers", REFS, ["explain", "helper"]),
    scenario("explain_cut", REFS, ["explain", "helper", "--limit", "1"]),
    scenario("explain_cut_json", REFS, ["explain", "helper", "--limit", "1", "--json"]),
    scenario("explain_limit_refused", PY, ["explain", "reorder_report", "--limit", "0"]),
]
PATH_SCENARIOS = [
    scenario("path_text", NESTED, ["path", "render_invoice", "format_amount"]),
    scenario("path_json", NESTED, ["path", "render_invoice", "format_amount", "--json"]),
    scenario("path_files", NESTED, ["path", "billing/invoice.py", "billing/money.py"]),
    scenario("path_backward", NESTED, ["path", "format_amount", "render_invoice"]),
    scenario("path_unresolved", PY, ["path", "nope", "zilch"]),
    scenario("path_unresolved_json", PY, ["path", "nope", "format_money", "--json"]),
    scenario("path_ambiguous_endpoint", REFS, ["path", "twin", "helper"]),
    scenario("path_none", REFS, ["path", "use", "helper"]),
    scenario("path_unique", REFS, ["path", "caller", "helper", "--include-unique"]),
    scenario(
        "path_ambiguous",
        REFS,
        ["path", "use", "py:a.py::twin", "--include-ambiguous", "--include-unique", "--json"],
    ),
    scenario(
        "path_exhausted", NESTED, ["path", "render_invoice", "format_amount", "--max-visited", "1"]
    ),
    scenario("path_visited_refused", NESTED, ["path", "a", "b", "--max-visited", "0"]),
]
COMMUNITIES = ["communities"]
COMMUNITY_SCENARIOS = [
    scenario("communities_text", NESTED, COMMUNITIES),
    scenario("communities_json", NESTED, [*COMMUNITIES, "--json"]),
    scenario("communities_cut", NESTED, [*COMMUNITIES, "--members", "1", "--limit", "1"]),
    scenario(
        "communities_cut_json", NESTED, [*COMMUNITIES, "--members", "2", "--limit", "1", "--json"]
    ),
    scenario("communities_resolution", NESTED, [*COMMUNITIES, "--resolution", "2.5"]),
    scenario("communities_flat", PY, [*COMMUNITIES, "--json"]),
    scenario("communities_tests_repo", TESTS, COMMUNITIES),
    scenario("communities_zero", NESTED, [*COMMUNITIES, "--resolution", "0"]),
    scenario("communities_nan", NESTED, [*COMMUNITIES, "--resolution", "nan"]),
    scenario("communities_high", NESTED, [*COMMUNITIES, "--resolution", "101"]),
    scenario("communities_members_refused", NESTED, [*COMMUNITIES, "--members", "0"]),
    scenario("communities_empty", NO_FILES, COMMUNITIES),
]
DIAGRAM_SCENARIOS = [
    scenario("diagram_text", NESTED, ["diagram"]),
    scenario("diagram_grouped", NESTED, ["diagram", "--communities"]),
    scenario("diagram_json", NESTED, ["diagram", "--json"]),
    scenario("diagram_references", REFS, ["diagram", "--communities"]),
    scenario("diagram_edge_bound", REFS, ["diagram", "--max-edges", "1"]),
    scenario("diagram_no_edges", REFS, ["diagram", "--max-edges", "0", "--json"]),
    scenario("diagram_node_bound", TESTS, ["diagram", "--max-nodes", "3", "--communities"]),
    scenario("diagram_node_bound_json", TESTS, ["diagram", "--max-nodes", "2", "--json"]),
    scenario("diagram_focus", NESTED, ["diagram", "--focus", "money.py"]),
    scenario("diagram_focus_symbol", NESTED, ["diagram", "--focus", " format_amount "]),
    scenario("diagram_focus_grouped", TESTS, ["diagram", "--focus", "pricing", "--communities"]),
    scenario("diagram_focus_miss", NESTED, ["diagram", "--focus", "zz_nothing"]),
    scenario("diagram_focus_several", REFS, ["diagram", "--focus", "twin"]),
    scenario("diagram_resolution_refused", NESTED, ["diagram", "--resolution", "-1"]),
    scenario("diagram_nodes_refused", NESTED, ["diagram", "--max-nodes", "0"]),
    scenario("diagram_check_match", committed(GROUPED), CHECK),
    scenario("diagram_check_fenced", committed(FENCED), CHECK),
    scenario("diagram_check_headed", committed(HEADED), CHECK),
    scenario("diagram_check_drift", committed(DRIFTED), CHECK),
    scenario("diagram_check_longer", committed(GROUPED + "\n"), CHECK),
    scenario("diagram_check_missing", NESTED, ["diagram", "--check", "nope.md"]),
]
HTML = ["html", "--cache-file"]
HTML_SCENARIOS = [
    scenario("html_cache_file", NESTED, [*HTML, "graph.html"]),
    scenario("html_cut", TESTS, [*HTML, "cut.html", "--max-nodes", "2", "--max-edges", "1"]),
    scenario("html_bad_name", NESTED, [*HTML, "../graph.html"]),
    scenario("html_long_name", NESTED, [*HTML, "a" * 124 + ".html"]),
    scenario("html_edges_refused", NESTED, ["html", "--max-edges", "5001"]),
    scenario("html_resolution_refused", NESTED, ["html", "--resolution", "inf"]),
]
HEALTH_SCENARIOS = [
    scenario("health_text", NESTED, ["health"]),
    scenario("health_json", PY, ["health", "--json"]),
    scenario("health_refs", REFS, ["health"]),
    scenario("health_tests_repo", TESTS, ["health"]),
    scenario("health_cut", TESTS, ["health", "--limit", "1"]),
    scenario("health_cut_json", TESTS, ["health", "--limit", "1", "--json"]),
    scenario("health_limit_refused", NESTED, ["health", "--limit", "501"]),
    scenario("health_empty", NO_FILES, ["health"]),
]
INDEX_SCENARIOS = [
    scenario("index_text", NESTED, ["index"]),
    scenario("index_json", NESTED, ["index", "--json"]),
    scenario("index_force", PY, ["index", "--force"]),
    scenario("index_repo_flag", NESTED, ["index", "--repo", "<SANDBOX>/repo"], "."),
]
SCENARIOS = [
    *EXPLAIN_SCENARIOS,
    *PATH_SCENARIOS,
    *COMMUNITY_SCENARIOS,
    *DIAGRAM_SCENARIOS,
    *HTML_SCENARIOS,
    *HEALTH_SCENARIOS,
    *INDEX_SCENARIOS,
]


def scenario_rows() -> list[object]:
    """Each scenario's answer, run eight at a time."""
    with ThreadPoolExecutor(max_workers=8) as pool:
        return list(pool.map(scenario_row, SCENARIOS))


def build() -> dict[Path, bytes]:
    files = {
        APP / "labels.json": labels_fixture(),
        RENDER / "fences.json": fences_fixture(),
        RENDER / "html.json": html_fixture(),
        CLI / "scenarios.json": {"runs": scenario_rows()},
    }
    return {path: render(sections) for path, sections in files.items()}


def main() -> int:
    refuse_real_caches()
    for directory in (RENDER, CLI):
        directory.mkdir(parents=True, exist_ok=True)
    return write_or_check_files(__doc__.splitlines()[0], APP, build)


if __name__ == "__main__":
    sys.exit(main())
