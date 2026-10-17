"""Write the app crate's write-surface fixtures: each input with the result Python gives.

Development-time only. Run it from the repository root, with the grammar libraries copied to a
scratch directory, so that no run reads or writes the user's grammar cache:

    TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy> uv run python scripts/app_write_fixtures.py

With --check, the script compares its output with the committed fixtures and writes nothing.
"""

import dataclasses
import json
import logging
import os
import re
import runpy
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path
from unittest import mock

from app_fixtures import CLONES, EROSION, apply, function, isolated, repo, summed
from fixture_files import Fixture, write_or_check

from agentless_mcp.application import lint_service
from agentless_mcp.application.lint_service import (
    LintCandidateInput,
    LintService,
    load_candidates,
    load_diff,
)
from agentless_mcp.application.patch_service import PatchService, load_edits
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import cache
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.util.errors import AgentlessError

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "app" / "fixtures" / "write"
GRAMMAR_OVERRIDE = "TREE_SITTER_LANGUAGE_PACK_CACHE_DIR"

APP = """\
def add(left, right):
    # Sum them.
    return left + right


def subtract(left, right):
    return left - right
"""

UTIL = 'VERSION = "1"\n'


def block(path: str, search: str, replace: str) -> str:
    """One SEARCH/REPLACE block for ``path``."""
    return f"### {path}\n<<<<<<< SEARCH\n{search}=======\n{replace}>>>>>>> REPLACE\n"


def edits_json(*edits: dict[str, object]) -> str:
    """An edits.json document of ``edits``."""
    return json.dumps({"edits": list(edits)})


ROUND = block("app.py", "    return left + right\n", "    return round(left + right, 2)\n")
SPACED = block("app.py", "    return left + right\n", "    return round(left+right,  2)\n")
BREAKING = block("app.py", "    return left + right\n", "    return round(left + right,\n")
VERSION = block("util.py", 'VERSION = "1"\n', 'VERSION = "2"\n')
MULTI = ROUND + "\n" + VERSION
COMMENT = block("app.py", "    # Sum them.\n", "    # Sum the two numbers together.\n")
TWO_IN_ONE = ROUND + block("app.py", "    return left - right\n", "    return left - right - 0\n")
CHAINED = block("app.py", "    return left + right\n", "    return MARKER\n") + block(
    "app.py", "    return MARKER\n", "    return left + right + 1\n"
)
MISSING = block("nowhere.py", "    return left + right\n", "    return 0\n")
ESCAPE = block("../outside.py", "secret\n", "leaked\n")
# Nothing below / on this path exists, so no symlink on any platform changes the resolved path.
ABSOLUTE = block("/agentless-absent/passwd", "root\n", "pwned\n")
HALF = ROUND + block("util.py", "NOT PRESENT ANYWHERE\n", 'VERSION = "2"\n')
EMPTY_SEARCH = edits_json({"path": "app.py", "search": "", "replace": "X"})
NOTES = block("notes.txt", "one  two\n", "one two three\n")
LATIN = APP + "# tail \xe9\n"
FFFD_OLD = 'VERSION = "1"  # � marker\n'
FFFD_NEW = 'VERSION = "2"  # � marker\n'
OVERSIZED = ["# " + "x" * 77 + "\n", 13_000]

BASE = repo({"app.py": APP, "util.py": UTIL, "notes.txt": "one  two\n"})
NESTED = repo({"sub/app.py": APP, "top.py": UTIL})


def op(
    name: str, kind: str, edits: str, steps: list[list[object]] = BASE, **extra: object
) -> dict[str, object]:
    """One operation case: the steps, the call, and its edits text."""
    return {"name": name, "steps": steps, "op": kind, "edits": edits, **extra}


CHECKS = [
    op("clean", "check", ROUND),
    op("breaking", "check", BREAKING),
    op("missing_file", "check", MISSING),
    op("two_spellings", "check", MULTI.replace("### app.py", "### ./app.py")),
    op("relative_escape", "check", ESCAPE),
    op("absolute_escape", "check", ABSOLUTE),
    op("no_grammar", "check", NOTES),
    op("not_utf8", "check", ROUND, [*BASE, ["bytes", "repo/app.py", LATIN]]),
    op("over_cap", "check", ROUND, [*BASE, ["repeat", "repo/app.py", *OVERSIZED]]),
    op("directory", "check", block("sub", "x\n", "y\n"), NESTED),
    op("the_root", "check", edits_json({"path": ".", "search": "x\n", "replace": "y\n"})),
    op("empty_search", "check", EMPTY_SEARCH),
    op("chained", "check", CHAINED),
    op("half_applied", "check", HALF),
    op(
        "already_broken",
        "check",
        block("bad.py", "x = 1\n", "x = 2\n"),
        [*BASE, ["write", "repo/bad.py", "def broken(:\nx = 1\n"]],
    ),
    op(
        "symlinked_path",
        "check",
        block("link.py", 'VERSION = "1"\n', 'VERSION = "3"\n'),
        [*BASE, ["symlink", "repo/link.py", "util.py"]],
    ),
]

NORMALIZES = [
    op("round", "normalize", ROUND),
    op("spaced", "normalize", SPACED),
    op("comment_only", "normalize", COMMENT),
    op("multi_file", "normalize", MULTI),
    op("text_only", "normalize", NOTES),
    op("half_applied", "normalize", HALF),
    op("escape", "normalize", ESCAPE),
    op("nothing_applied", "normalize", MISSING),
]

WORKTREE = {"mode": "worktree"}
IN_PLACE = {"mode": "in_place"}
DIRTY = [*BASE, ["write", "repo/app.py", APP + "\n# scratch\n"]]
PLAIN = [["mkdir", "plain"], ["write", "plain/app.py", APP]]
UNTRACKED = [*BASE, ["write", "repo/extra/new.py", "x = 1\n"]]

APPLIES = [
    op("worktree_diff", "apply", ROUND, **WORKTREE),
    op("worktree_multi_file", "apply", MULTI, **WORKTREE),
    op("worktree_two_in_one", "apply", TWO_IN_ONE, **WORKTREE),
    op("worktree_chained", "apply", CHAINED, **WORKTREE),
    op("worktree_missing_file", "apply", MISSING, **WORKTREE),
    op("worktree_half_applied", "apply", HALF, **WORKTREE),
    op("worktree_escape", "apply", ESCAPE, **WORKTREE),
    op("worktree_no_git", "apply", ROUND, PLAIN, root="plain", **WORKTREE),
    op("worktree_of_a_subdirectory", "apply", ROUND, NESTED, root="repo/sub", **WORKTREE),
    op("worktree_untracked_root", "apply", ROUND, UNTRACKED, root="repo/extra", **WORKTREE),
    op("worktree_dirty_is_allowed", "apply", ROUND, DIRTY, **WORKTREE),
    op("in_place_clean", "apply", MULTI, **IN_PLACE),
    op("in_place_dirty", "apply", ROUND, DIRTY, **IN_PLACE),
    op("in_place_unknown_state", "apply", ROUND, PLAIN, root="plain", **IN_PLACE),
    op(
        "in_place_not_utf8",
        "apply",
        ROUND,
        [*BASE, ["bytes", "repo/app.py", APP + "# tail \xe9\n"], ["commit", "repo"]],
        **IN_PLACE,
    ),
    op(
        "in_place_real_fffd",
        "apply",
        block("util.py", FFFD_OLD, FFFD_NEW),
        [*BASE, ["write", "repo/util.py", FFFD_OLD], ["commit", "repo"]],
        **IN_PLACE,
    ),
    op(
        "in_place_crlf",
        "apply",
        block("util.py", "BUILD = 7\r\n", "BUILD = 8\r\n"),
        [*BASE, ["bytes", "repo/util.py", 'VERSION = "1"\r\nBUILD = 7\r\n'], ["commit", "repo"]],
        **IN_PLACE,
    ),
    op("in_place_empty_search", "apply", EMPTY_SEARCH, **IN_PLACE),
    op("in_place_half_applied", "apply", HALF, **IN_PLACE),
    op("in_place_escape", "apply", ESCAPE, DIRTY, **IN_PLACE),
    op(
        "in_place_symlinked_path",
        "apply",
        block("link.py", 'VERSION = "1"\n', 'VERSION = "3"\n'),
        [*BASE, ["symlink", "repo/link.py", "util.py"], ["commit", "repo"]],
        **IN_PLACE,
    ),
    op("in_place_read_only", "apply", ROUND, [*BASE, ["chmod", "repo", 0o555]], **IN_PLACE),
]


class _Warnings(logging.Handler):
    def __init__(self) -> None:
        super().__init__(logging.WARNING)
        self.lines: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.lines.append(record.getMessage())


# Each file below the repository outside .git, as latin-1 text so that every byte survives.
def _tree(root: Path) -> dict[str, str]:
    if not root.is_dir():
        return {}
    files = sorted(path for path in root.rglob("*") if ".git" not in path.parts and path.is_file())
    return {
        path.relative_to(root).as_posix(): path.read_bytes().decode("latin-1") for path in files
    }


def _called(case: dict[str, object], root: Path) -> object:
    service = PatchService(TreeSitterExtractor())
    edits = load_edits(str(case["edits"])).edits
    ctx = resolve_repo(root, None)
    if case["op"] == "check":
        return service.check(edits, ctx)
    if case["op"] == "normalize":
        return service.normalize(edits, ctx)
    return service.apply(edits, ctx, in_place=case["mode"] == "in_place")


def operation_row(case: dict[str, object]) -> dict[str, object]:
    """Run one operation case in a scratch sandbox and record its result and the tree after it."""
    with tempfile.TemporaryDirectory(prefix="app-write-") as made:
        sandbox = Path(made).resolve()
        with isolated(sandbox):
            modes = [step for step in case["steps"] if step[0] == "chmod"]
            for step in case["steps"]:
                if step[0] != "chmod":
                    apply(sandbox, step)
            for _, place, mode in modes:
                (sandbox / str(place)).chmod(int(str(mode)))
            root = sandbox / str(case.get("root", "repo"))
            warnings = _Warnings()
            logging.getLogger().addHandler(warnings)
            try:
                report = _called(case, root)
                expected: dict[str, object] = {"report": json.dumps(report.as_dict())}
            except AgentlessError as refused:
                expected = {"error": [type(refused).__name__, str(refused)]}
            finally:
                logging.getLogger().removeHandler(warnings)
                for _, place, _mode in modes:
                    (sandbox / str(place)).chmod(0o755)
            tree = _tree(sandbox / "repo") if case["op"] == "apply" else {}
        shown = json.dumps(expected).replace(str(sandbox), "<SANDBOX>")
        lines = [line.replace(str(sandbox), "<SANDBOX>") for line in warnings.lines]
    return {**case, "expected": json.loads(shown), "diagnostics": lines, "tree": tree}


def blocks_text(count: int, rows: int) -> str:
    """Raw SEARCH/REPLACE text of ``count`` blocks with ``rows`` one-character lines each."""
    old = "\n".join("x" for _ in range(rows))
    new = "\n".join("y" for _ in range(rows))
    return "".join(
        f"### pkg/mod_{index:04d}.py\n<<<<<<< SEARCH\n{old}\n=======\n{new}\n>>>>>>> REPLACE\n\n"
        for index in range(count)
    )


def json_text(count: int, rows: int) -> str:
    """The same request in the edits.json form."""
    old = "\n".join("x" for _ in range(rows))
    new = "\n".join("y" for _ in range(rows))
    edits = [
        {"index": index, "path": f"pkg/mod_{index:04d}.py", "search": old, "replace": new}
        for index in range(count)
    ]
    return json.dumps({"edits": edits})


RECIPES: dict[str, Callable[[int, int], str]] = {"blocks": blocks_text, "json": json_text}

LOAD_TEXTS = [
    MULTI,
    "  \n" + edits_json({"path": "a.py", "search": "x", "replace": "y"}),
    edits_json({"path": "a.py", "search": "x", "replace": "y", "index": 7}),
    edits_json({"path": "a.py", "search": "x", "replace": "y", "index": -5}),
    edits_json(
        {"path": "a.py", "search": "x", "replace": "y"},
        {"path": "b.py", "search": "1", "replace": "2", "index": 0},
    ),
    edits_json(),
    '{"edits": [], "edits": [{"path": "b.py", "search": "x", "replace": "y"}]}',
    '{"edits": [{"path": "a.py", "path": "c.py", "search": "x", "replace": "y"}]}',
    '{"edits": [{"path": "a.py", "replace": "x"}]}',
    '{"edits": [{"search": "x", "replace": "y"}]}',
    '{"edits": [{"path": 1, "search": "x", "replace": "y"}]}',
    '{"blocks": []}',
    "{}",
    '{"edits": [',
    '{"edits": [1, ]}',
    '{"edits": {"path": "a.py"}}',
    '{"edits": ["### a.py"]}',
    '{"edits": [null]}',
    '{"edits": [{"path": "a.py", "search": "x", "replace": "y", "index": "1"}]}',
    '{"edits": [{"path": "a.py", "search": "x", "replace": "y", "index": 1.5}]}',
    '{"edits": [{"path": "a.py", "search": "x", "replace": "y", "index": null}]}',
    '{"edits": [{"path": "a.py", "search": "x", "replace": "y", "index": 1e3}]}',
    '{"edits": [{"path": "a\\u00e9.py", "search": "x\\n", "replace": "y\\n"}]} trailing',
    "[1]",
    "﻿" + edits_json(),
    "\x0b" + edits_json(),
    "no blocks at all",
    "",
]

LOAD_RECIPES = [
    ["blocks", 501, 1],
    ["json", 501, 1],
    ["blocks", 100, 11_000],
    ["json", 100, 11_000],
    ["json", 500, 1000],
    ["blocks", 500, 1],
]


def loaded(text: str) -> dict[str, object]:
    """The edits document that ``text`` loads to, or the refusal text."""
    try:
        return {"parsed": json.dumps(load_edits(text).as_dict())}
    except AgentlessError as refused:
        return {"error": str(refused)}


def counted(text: str) -> dict[str, object]:
    """The number of edits that ``text`` loads to, or the refusal text."""
    try:
        return {"edits": len(load_edits(text).edits)}
    except AgentlessError as refused:
        return {"error": str(refused)}


def load_fixture() -> Fixture:
    """Every load_edits case: literal texts, and texts built by a recipe for the bounds."""
    recipes = [
        [*recipe, counted(RECIPES[str(recipe[0])](int(recipe[1]), int(recipe[2])))]
        for recipe in LOAD_RECIPES
    ]
    return {"texts": [[text, loaded(text)] for text in LOAD_TEXTS], "recipes": recipes}


def patch_fixture() -> Fixture:
    """Every check, normalize and apply case, each run in its own sandbox."""
    if not os.environ.get(GRAMMAR_OVERRIDE):
        sys.exit(f"refused: set {GRAMMAR_OVERRIDE} to a scratch copy of the grammar libraries")
    os.environ["AGENTLESS_MCP_NO_DOWNLOAD"] = "1"
    os.environ["AGENTLESS_MCP_NO_AUTO_WARM"] = "1"
    return {
        "check": [operation_row(case) for case in CHECKS],
        "normalize": [operation_row(case) for case in NORMALIZES],
        "apply": [operation_row(case) for case in APPLIES],
    }


ADD = "def add(left, right):\n    return left + right\n"
LINTED = repo({"app.py": ADD, "util.py": UTIL})
GOOD = block("app.py", "    return left + right\n", "    return round(left + right, 2)\n")
ESCAPING = block("../outside.py", "root\n", "pwned\n")
CROSSING = block("app.py", function("handle", 10), function("handle", 11))
PAIRS_ONLY = ["complexity_delta"]
CLONE_ONLY = ["clone_delta"]
OVERLAP = ["near_duplicates", "clone_delta"]
CORE = '"""Core."""\n\nRATE = 3\n\n\ndef quote(sku):\n    return RATE\n\n\nclass PriceBook:\n'
CORE += "    def cost_of(self, sku):\n        return quote(sku)\n"
CALLER = "from core import quote\n\n\ndef run_billing(items):\n"
CALLER += "    return sum(quote(item) for item in items)\n"
PYPROJECT = '[project]\nname = "fixture"\nversion = "0"\ndependencies = []\n'
BILLING = repo({"core.py": CORE, "caller.py": CALLER, "pyproject.toml": PYPROJECT})
BRANCH_DIFF = (
    "diff --git a/core.py b/core.py\nindex 1111111..2222222 100644\n--- a/core.py\n+++ b/core.py\n"
    '@@ -1,4 +1,6 @@\n """Core."""\n \n+import yaml\n+\n RATE = 3\n \n'
    "@@ -5,7 +7,7 @@\n \n \n def quote(sku):\n-    return RATE\n+    return compute_rate(sku)\n \n"
    " \n class PriceBook:\n"
)
APPLIED_DIFF = (
    "--- a/core.py\n+++ b/core.py\n@@ -6,2 +6,2 @@\n"
    "-def quote(sku):\n-    return SOMETHING_ELSE\n+def quote(sku):\n+    return RATE\n"
)
MIXED_DIFF = (
    "diff --git a/logo.png b/logo.png\nindex 1234567..89abcde 100644\n"
    "Binary files a/logo.png and b/logo.png differ\n"
    "diff --git a/core.py b/core.py\n--- a/core.py\n+++ b/core.py\n@@ -7,1 +7,1 @@\n"
    "-    return RATE\n+    return compute_rate(sku)\n"
)
RENAME_DIFF = (
    "diff --git a/core.py b/renamed.py\nsimilarity index 90%\nrename from core.py\n"
    "rename to renamed.py\n"
)
MODE_DIFF = "diff --git a/core.py b/core.py\nold mode 100644\nnew mode 100755\n"
LOOP_REPO = repo({"app.py": summed("total"), "lib.py": "VALUE = 1\n"}, name="repo")
CONFIGURED = repo(
    {"app.py": function("handle", 3), ".agentless-mcp.json": '{"complexity_threshold": 3}'}
)
ADD_ONE = edits_json({"path": "app.py", "search": ADD, "replace": ADD + "\nx = 1\n"})


def lint_case(
    name: str, steps: list[list[object]], target: str = "cands", **extra: object
) -> dict[str, object]:
    """One candidate-form case: the steps, the input path, and the options."""
    return {"name": name, "steps": steps, "target": target, **extra}


def patches(*texts: str) -> list[list[object]]:
    """The steps that write each text as one candidate file of cands/."""
    return [["write", f"cands/{index:02}.patch", text] for index, text in enumerate(texts)]


def both_modes(name: str, steps: list[list[object]], text: str) -> list[dict[str, object]]:
    """The case with every check, and with the one pair check that needs no scan."""
    steps = [*steps, *patches(text)]
    return [lint_case(name, steps), lint_case(f"{name}_pairs_only", steps, checks=PAIRS_ONLY)]


def diff_case(name: str, steps: list[list[object]], text: str) -> dict[str, object]:
    """One diff-form case."""
    return lint_case(name, [*steps, ["write", "change.diff", text]], "change.diff", form="diff")


CANDIDATES = [
    lint_case("refused_costs_only_itself", [*LINTED, *patches(GOOD, ESCAPING)]),
    lint_case("good", [*LINTED, *patches(GOOD)]),
    *both_modes("crossing", EROSION, CROSSING),
    lint_case("cycle_delta_subset", [*EROSION, *patches(CROSSING)], checks=["cycle_delta"]),
    *both_modes("creates_a_file", EROSION, block("pkg/new.py", "", function("fresh", 12))),
    *both_modes("empty_search_on_empty_file", EROSION, block("empty.py", "", function("x", 12))),
    *both_modes("does_not_apply", EROSION, block("twice.py", "x = 1\n", "x = 2\n")),
    *both_modes(
        "over_the_cap",
        [*EROSION, ["repeat", "repo/huge.py", "y = 1\n", 200_000]],
        block("huge.py", "y = 1\n", "y = 2\n"),
    ),
    *both_modes(
        "not_utf8",
        [*EROSION, ["bytes", "repo/latin.py", "y = 1\nx = '\xff'\n"]],
        block("latin.py", "y = 1\n", "y = 2\n"),
    ),
    *both_modes("escape", EROSION, ESCAPING),
    *both_modes(
        "misapplied_scanned_file",
        EROSION,
        CROSSING + "\n" + block("config.yaml", "missing: line\n", "other: line\n"),
    ),
    lint_case(
        "configured_threshold",
        [*CONFIGURED, *patches(block("app.py", function("handle", 3), function("handle", 4)))],
        checks=PAIRS_ONLY,
    ),
    lint_case("malformed_block", [*LINTED, *patches(GOOD + "### app.py\n<<<<<<< SEARCH\nx\n")]),
    lint_case("no_edits", [*LINTED, *patches('{"edits": []}')]),
    lint_case("edits_json", [*LINTED, *patches(ADD_ONE)]),
    lint_case("a_stray_readme", [*LINTED, ["write", "cands/README", "read me\n"], *patches(GOOD)]),
    lint_case("one_file", [*LINTED, *patches(GOOD)], "cands/00.patch"),
    lint_case("missing_target", LINTED, "nowhere"),
    lint_case("empty_directory", [*LINTED, ["mkdir", "cands"]]),
    lint_case("unknown_check", [*LINTED, *patches(GOOD)], checks=["complexity"]),
    lint_case("no_check", [*LINTED, *patches(GOOD)], checks=[]),
    lint_case(
        "copies_out_of_scope",
        [
            *CLONES,
            *patches(
                block("tests/test_more.py", "", summed("more"))
                + block("fixtures/more.py", "", summed("more"))
            ),
        ],
        checks=CLONE_ONLY,
    ),
    lint_case(
        "one_shared_index",
        [*CLONES, *patches(*(block(f"lib{n}.py", "", summed(f"again{n}")) for n in range(3)))],
        checks=CLONE_ONLY,
    ),
    lint_case(
        "index_budget_spent",
        [*CLONES, *patches(block("lib.py", "", summed("again")))],
        checks=CLONE_ONLY,
        budget="zero",
    ),
    lint_case(
        "unreadable_repository_file",
        [
            *CLONES,
            ["bytes", "repo/latin.py", "y = 1\nx = '\xff'\n"],
            *patches(block("lib.py", "", summed("again"))),
        ],
        checks=CLONE_ONLY,
    ),
    lint_case(
        "near_duplicates_overlap",
        [*LOOP_REPO, *patches(block("lib.py", "VALUE = 1\n", "VALUE = 1\n\n\n" + summed("again")))],
        checks=OVERLAP,
    ),
    lint_case(
        "created_file_overlap_gap",
        [*CLONES, *patches(block("lib.py", "", summed("again")))],
        checks=OVERLAP,
    ),
    lint_case("exact_copy", [*CLONES, *patches(block("lib.py", "", summed("total")))]),
    lint_case(
        "undeclared_import",
        [*BILLING, *patches(block("core.py", '"""Core."""\n', '"""Core."""\n\nimport yaml\n'))],
    ),
    lint_case(
        "manifest_shape_warning",
        [
            *repo({"app.py": ADD, "pyproject.toml": '[project]\nname = "x"\ndependencies = "x"\n'}),
            *patches(GOOD),
        ],
    ),
    lint_case(
        "manifest_outside_the_root",
        [
            *LINTED,
            ["write", "outside.toml", '[project]\nname = "x"\ndependencies = ["leak"]\n'],
            ["symlink", "repo/pyproject.toml", "<SANDBOX>/outside.toml"],
            *patches(GOOD),
        ],
    ),
    diff_case("branch_diff", BILLING, BRANCH_DIFF),
    diff_case("already_applied_diff", BILLING, APPLIED_DIFF),
    diff_case("binary_and_code", BILLING, MIXED_DIFF),
    diff_case("refused_rename", BILLING, RENAME_DIFF),
    diff_case("mode_only", BILLING, MODE_DIFF),
    lint_case("missing_diff", BILLING, "nope.patch", form="diff"),
]


def _inputs(case: dict[str, object], sandbox: Path) -> tuple[object, ...]:
    target = sandbox / str(case["target"])
    if case.get("form") == "diff":
        return (load_diff(target),)
    return load_candidates(target)


def _linted(case: dict[str, object], sandbox: Path) -> dict[str, object]:
    extractor = TreeSitterExtractor()
    ctx = resolve_repo(sandbox / str(case.get("root", "repo")), None)
    ctx = dataclasses.replace(ctx, symbols=cache.OnDemandSource(extractor))
    checks = case.get("checks")
    names = lint_service.CHECK_NAMES if checks is None else tuple(checks)
    try:
        view = LintService(extractor).lint(ctx, _inputs(case, sandbox), names)
    except AgentlessError as refused:
        return {"error": [type(refused).__name__, str(refused)]}
    return {"report": json.dumps(view.as_dict())}


def lint_row(case: dict[str, object]) -> dict[str, object]:
    """Run one candidate-form case in a scratch sandbox and record its report and warnings."""
    with tempfile.TemporaryDirectory(prefix="app-write-") as made:
        sandbox = Path(made).resolve()
        with isolated(sandbox):
            for step in case["steps"]:
                apply(sandbox, step)
            spent = case.get("budget") == "zero"
            budget = (lambda _left: 0.0) if spent else lint_service.deadline.walk_budget
            warnings = _Warnings()
            logging.getLogger().addHandler(warnings)
            try:
                with mock.patch.object(lint_service.deadline, "walk_budget", budget):
                    expected = _linted(case, sandbox)
            finally:
                logging.getLogger().removeHandler(warnings)
        shown = json.dumps(expected).replace(str(sandbox), "<SANDBOX>")
        lines = [line.replace(str(sandbox), "<SANDBOX>") for line in warnings.lines]
    # The approved non-UTF-8 rule: the Rust build names a byte that is not UTF-8 with U+FFFD.
    shown = re.sub(r"\\udc[89a-f][0-9a-f]", r"\\ufffd", shown)
    return {**case, "expected": json.loads(shown), "diagnostics": lines}


DECLARED = (
    '[project]\nname = "x"\nrequires-python = ">=3.10"\ndependencies = ["Requests>=2", "py_yaml"]\n'
)
SHAPES = (
    '[project]\nname = "x"\nrequires-python = 3\ndependencies = ["a", 1]\n'
    '[project.optional-dependencies]\ndocs = "b"\n[dependency-groups]\ndev = ["c"]\n'
)

MANIFESTS = [
    lint_case("none", LINTED),
    lint_case("pyproject", repo({"pyproject.toml": DECLARED})),
    lint_case(
        "pyproject_outside",
        [
            ["mkdir", "repo"],
            ["write", "outside.toml", '[project]\nname = "x"\ndependencies = ["leak"]\n'],
            ["symlink", "repo/pyproject.toml", "<SANDBOX>/outside.toml"],
        ],
    ),
    lint_case(
        "requirements_outside",
        [
            ["mkdir", "repo"],
            ["write", "outside.txt", "leak\n"],
            ["symlink", "repo/requirements.txt", "<SANDBOX>/outside.txt"],
        ],
    ),
    lint_case(
        "symlinked_inside",
        [
            ["write", "repo/real.toml", '[project]\nname = "x"\ndependencies = ["declared"]\n'],
            ["symlink", "repo/pyproject.toml", "<SANDBOX>/repo/real.toml"],
        ],
    ),
    lint_case(
        "requirements_files",
        [
            ["write", "repo/requirements.txt", "alpha\n# note\nbeta==1 ; python_version > '3'\n"],
            ["write", "repo/requirements-dev.txt", "-r requirements.txt\ngamma\n"],
            ["mkdir", "repo/requirements-dir.txt"],
            ["write", "repo/requirementstxt", "never\n"],
        ],
    ),
    # Alone in its case, because a case-insensitive file system holds one of the two spellings.
    lint_case("requirements_capital_letter", [["write", "repo/Requirements.txt", "never\n"]]),
    lint_case(
        "requirements_over_the_cap",
        [["repeat", "repo/requirements.txt", "alpha\n", 200_000]],
    ),
    lint_case(
        "requirements_name_order",
        [
            ["write", "repo/requirements-\xe9.txt", "accented\n"],
            ["latin_name", "repo/requirements-\x80.txt", "raw\n"],
            ["write", "repo/requirements-z.txt", "zed\n"],
        ],
    ),
    lint_case("shape_warnings", [["write", "repo/pyproject.toml", SHAPES]]),
]


def manifest_row(case: dict[str, object]) -> dict[str, object]:
    """Read one case's manifests in a scratch sandbox."""
    with tempfile.TemporaryDirectory(prefix="app-write-") as made:
        sandbox = Path(made).resolve()
        with isolated(sandbox):
            for step in case["steps"]:
                apply(sandbox, step)
            declared = lint_service.read_declared_dependencies(sandbox / "repo")
    expected = {
        "packages": sorted(declared.packages),
        "sources": list(declared.sources),
        "warnings": list(declared.warnings),
        "requires_python": declared.requires_python,
    }
    shown = re.sub(r"\\udc[89a-f][0-9a-f]", r"\\ufffd", json.dumps(expected))
    return {**case, "expected": json.loads(shown)}


STEMS = ["a.patch", "a.", ".a", "a.b.c", "..", ".", "a..", ".a.b", "x", "dir/b.diff", "a/.", ""]

ROOT = Path(__file__).resolve().parents[1]
GOLDEN_TESTS = ROOT / "tests" / "characterization" / "test_lint_goldens.py"
GOLDEN_IDS = {
    "01-renames": "RENAMING_CANDIDATE",
    "02-calls": "CALLING_CANDIDATE",
    "03-branches": "BRANCHING_CANDIDATE",
    "04-defaults": "DEFAULTS_CANDIDATE",
    "05-copies": "COPYING_CANDIDATE",
}


def golden_row() -> dict[str, object]:
    """The lint golden's five candidates, its boundary config and its repository files.

    The report of the candidates over a scratch copy of the repository must equal the golden's
    candidates, or the copy is not the golden's input and the script refuses.
    """
    golden = runpy.run_path(str(GOLDEN_TESTS))
    fixtures = golden["FIXTURES"] / "repo_py"
    files = {path.name: path.read_text(encoding="utf-8") for path in sorted(fixtures.glob("*.py"))}
    candidates = [[name, golden[attribute]] for name, attribute in GOLDEN_IDS.items()]
    globs = list(golden["BOUNDARY"].boundary_globs)
    with tempfile.TemporaryDirectory(prefix="app-write-") as made:
        root = Path(made).resolve() / "repo"
        root.mkdir()
        for name, text in files.items():
            (root / name).write_text(text, encoding="utf-8")
        ctx = dataclasses.replace(golden["context_for"]("repo_py"), root=root)
        inputs = [LintCandidateInput(id=name, parsed=load_edits(text)) for name, text in candidates]
        view = LintService(TreeSitterExtractor()).lint(ctx, inputs)
    shown = json.loads(json.dumps(view.as_dict()))["candidates"]
    committed = json.loads(golden["golden_path"]("lint.json").read_text(encoding="utf-8"))
    if shown != committed["candidates"]:
        sys.exit("refused: a scratch copy of repo_py does not give the lint golden's candidates")
    return {"candidates": candidates, "boundary_globs": globs, "files": files}


def lint_fixture() -> Fixture:
    """Every candidate-form case, the manifest reads, and the stem rule of an input's id."""
    if not os.environ.get(GRAMMAR_OVERRIDE):
        sys.exit(f"refused: set {GRAMMAR_OVERRIDE} to a scratch copy of the grammar libraries")
    os.environ["AGENTLESS_MCP_NO_DOWNLOAD"] = "1"
    os.environ["AGENTLESS_MCP_NO_AUTO_WARM"] = "1"
    return {
        "candidates": [lint_row(case) for case in CANDIDATES],
        "manifests": [manifest_row(case) for case in MANIFESTS],
        "stems": [[name, Path(name).stem] for name in STEMS],
        "golden": [golden_row()],
    }


BUILDERS: dict[str, Callable[[], Fixture]] = {
    "edits": load_fixture,
    "lint": lint_fixture,
    "patch": patch_fixture,
}


if __name__ == "__main__":
    raise SystemExit(write_or_check((__doc__ or "").splitlines()[0], FIXTURES, BUILDERS))
