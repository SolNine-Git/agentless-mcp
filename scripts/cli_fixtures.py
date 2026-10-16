"""Write the cli crate's test fixtures: what the Python command line prints for each command line.

Development-time only. Run it from the repository root, with the grammar libraries copied to a
scratch directory, so that no run reads or writes the user's grammar cache:

    TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy>/v1.14.3/libs \
        TIKTOKEN_CACHE_DIR=<scratch copy of the cl100k_base file> \
        uv run python scripts/cli_fixtures.py

With --check, the script compares its output with the committed fixtures and writes nothing.

Each case runs the console script's entry point in a fresh interpreter, with HOME, the caches and
the git config inside a temporary sandbox. The 'tokens' extra is installed, as the Rust build always
has the cl100k_base counter, and tiktoken reads the encoding from TIKTOKEN_CACHE_DIR.
"""

import argparse
import importlib.metadata
import os
import re
import subprocess
import sys
import tempfile
import textwrap
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from fixture_files import (
    Fixture,
    Identity,
    hermetic_env,
    refuse_real_caches,
    run_git,
    write_or_check,
)

from agentless_mcp.adapters.cli.main import build_parser
from agentless_mcp.core import grammars

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "cli" / "fixtures"
GRAMMAR_OVERRIDE = "TREE_SITTER_LANGUAGE_PACK_CACHE_DIR"
NOWHERE = "/nonexistent/agentless-cli-fixture"
VERSION = importlib.metadata.version("agentless-mcp")
TIMEOUT_SECONDS = 60

BOOT = """
from agentless_mcp.bootstrap import cli_main
raise SystemExit(cli_main())
"""

IDENTITY = Identity("cli tests", "cli@example.invalid", "2026-02-01T00:00:00Z")

# Texts that pin the wrap rules beyond the help strings: hyphens, dash runs, long words.
WRAP_EXTRAS = [
    "Look, goof-ball -- use the -b option!",
    "a-b-c x-ray tier-1-2 --flag-name ---x end.--next word--word",
    "supercalifragilisticexpialidocious-and-then-some more-words-with-hyphens-in-them",
    "--------------------------------------------- dashes-------only",
    "a" * 70 + "-" + "b" * 70,
    "short",
]

HELP_ARGVS = [
    ["--help"],
    ["lint", "--help"],
    ["capabilities", "--help"],
    ["lint"],
    ["nope"],
    ["capabilities", "--repo"],
    ["patch", "--help"],
    ["patch", "parse", "--help"],
    ["patch", "check", "--help"],
    ["patch", "apply", "--help"],
    ["patch", "normalize", "--help"],
    ["validate", "--help"],
    ["vote", "--help"],
    ["patch"],
    ["validate"],
    ["vote"],
]
WIDTHS = [None, "1", "10", "20", "40", "60", "79", "80", "81", "100", "120", "200", "1000"]
ODD_COLUMNS = [
    *["abc", "0", "-5", " 90 ", "1_00", "+70", "", "1__0", "_1"],
    # int() strips no U+001C..U+001F, which str.isspace counts, and it strips U+0085.
    *["\x1c40", "40\x1f", "\x8540"],
]

CORPUS: list[list[str]] = [
    [],
    ["lint", "--path", "a.py"],
    ["lint", "--path", "a.py", "--since", "HEAD"],
    ["lint", "--since", "HEAD", "--after", "x"],
    ["lint", "--path", "a.py", "--only", "arity", "--after", "x"],
    ["lint", "--since", "HEAD", "--only", "arity"],
    ["lint", "--since", "HEAD", "--only", "nope"],
    ["lint", "--since", "HEAD", "--only", "arity,"],
    ["lint", "--since", "HEAD", "--only", ""],
    ["lint", "--since", "HEAD", "--fail-on", "nope"],
    ["lint", "--since", "HEAD", "--fail-on", "warning,bad"],
    ["lint", "--since"],
    ["lint", "--bogus"],
    ["lint", "--since", "HEAD", "--token-counter", "chars4"],
    ["--token-counter", "bogus", "capabilities"],
    ["--token-counter=tiktoken", "capabilities", "--repo", NOWHERE],
    ["capabilities", "--repo", NOWHERE],
    ["capabilities", "extra"],
    ["lint", "--since", "HEAD", "--re", NOWHERE],
    ["--h"],
    ["--no", "capabilities", "--repo", NOWHERE],
    ["lint", "--since=HEAD", f"--repo={NOWHERE}"],
    ["lint", "--since", "HEAD", "--since", "HEAD~1", "--repo", NOWHERE],
    ["lint", "--after", "x", "--candidates", "y", "--repo", NOWHERE],
    ["lint", "-h", "--bogus"],
    ["lint", "--since", "HEAD", "--", "--repo"],
    ["capabilities", "--json", "--json", "--repo", NOWHERE],
    ["lint", "--since", "HEAD", "--only", "complexity_delta,", "clone_delta"],
    ["capa"],
    ["lint", "--bogus", "--since", "HEAD", "--repo", NOWHERE],
    ["lint", "--bogus", "--only", "nope"],
    ["lint", "--only", "nope", "--path", "a", "--since", "b"],
    ["lint", "--path", "a", "--since", "b", "--only", "nope"],
    ["capabilities", "--no-auto-warm"],
    ["--repo", "x", "lint"],
    ["lint", "--since", "HEAD", "-x", "--repo", NOWHERE],
    ["lint", "--json=1", "--since", "HEAD"],
    ["lint", "--json=", "--since", "HEAD"],
    ["--help=foo"],
    ["lint", "--help=1"],
    ["lint", "-h=1"],
    ["-hx"],
    ["-hh"],
    ["-h-x"],
    ["lint", "--since", "-1", "--repo", NOWHERE],
    ["lint", "--since", "-1.5", "--repo", NOWHERE],
    ["lint", "--since", "-x"],
    ["lint", "--path", "-a b", "--after", "x", "--repo", NOWHERE],
    ["--=x"],
    ["-=x"],
    ["lint", "--=x"],
    ["-"],
    ["lint", "--since", "HEAD", "-"],
    ["--", "lint", "--since", "HEAD", "--repo", NOWHERE],
    ["lint", "--", "--since", "HEAD"],
    ["--token-counter", "tiktoken", "lint", "--path", "a.py"],
    ["--token-counter", "tiktoken", "nope"],
    ["--token-counter", "chars4", "--token-counter", "tiktoken", "capabilities", "--repo", NOWHERE],
    ["--no-auto-warm", "--no-auto-warm", "capabilities", "--repo", NOWHERE],
    ["--tok=chars4", "capabilities", "--repo", NOWHERE],
    ["--token-counter"],
    ["--token-counter", "--no-auto-warm", "lint"],
    ["--token-counter", "lint"],
    ["--json", "capabilities", "--repo", NOWHERE],
    ["--no-auto-warm"],
    ["-h", "nope"],
    ["nope", "-h"],
    [
        *["lint", "--since", "HEAD", "--only", " complexity_delta , complexity_delta"],
        *["--repo", NOWHERE],
    ],
    ["lint", "--since", "HEAD", "--fail-on", "", "--repo", NOWHERE],
    ["lint", "--since", "HEAD", "--fail-on", "advisory, warning", "--repo", NOWHERE],
    ["lint", "--path", "a", "--after", "b", "--since", "c"],
    ["lint", "--path", "a", "--after", "b", "--only", "complexity_delta,arity,shadowing"],
    ["lint", "--since", "HEAD", "--only", "clone_delta,arity", "--after", "x"],
    ["lint", "--sinc=HEAD", f"--repo={NOWHERE}"],
    ["lint", "--since", "HEAD", "--only=complexity_delta", "--fail-on=advisory", "--repo", NOWHERE],
    ["lint", "--path", "a", "--after"],
    ["lint", "--since", "HEAD", "--only", "clone_delta", "--only", "nope"],
    ["lint", "--path", "a", "--candidates", "b"],
    ["lint", "--candidates", "b", "--diff", "c"],
    ["lint", "--path", "a", "--path", "b", "--after", "x", "--repo", NOWHERE],
    ["lint", "--path", "a", "--after", "b", "--since"],
    ["lint", "--since", "HEAD", "--only", "Complexity_delta"],
    ["capabilities", "-x"],
    ["capabilities", "--repo", NOWHERE, "--no-cache", "--json"],
    ["lint", "--candidates", "candidates-dir", "--fail-on", "error"],
    ["lint", "--candidates", "c", "--only", "nope"],
    ["lint", "--candidates", "c", "--only", "arity,"],
    ["lint", "--candidates", "c", "--after", "x.py"],
    ["lint", "--path", "core.py", "--after", "after.py", "--only", "complexity_delta,arity"],
    ["lint", "--candidates", "c", "--path", "core.py", "--after", "x.py"],
    ["lint", "--since", "HEAD", "--candidates", "c"],
    ["teleport", "--repo", "r"],
    # The write subcommands: usage errors, nested choices and the inputs a run refuses.
    ["patch", "bogus"],
    ["patch", "pa"],
    ["--bogus", "patch", "--bogus"],
    ["patch", "parse", "--bogus"],
    ["patch", "--bogus", "parse"],
    ["patch", "parse", "-f"],
    ["patch", "parse", "-f", NOWHERE, "--bogus"],
    ["patch", "parse", "-f", NOWHERE],
    ["patch", "parse", f"-f{NOWHERE}"],
    ["patch", "parse", f"--fi={NOWHERE}"],
    ["patch", "parse", "-f", NOWHERE, "-f", f"{NOWHERE}/x//"],
    ["patch", "-h", "parse"],
    ["patch", "parse", "-h", "--bogus"],
    ["patch", "check", "--in-place"],
    ["patch", "check", "--no-cache"],
    ["patch", "apply", "--in-place=1"],
    ["patch", "apply", "--in", "--repo", NOWHERE, "-f", NOWHERE],
    ["patch", "check", "-f", NOWHERE, "--repo", NOWHERE],
    ["patch", "normalize", "--json", "--json", "--repo", NOWHERE],
    ["patch", "parse", "check"],
    ["--token-counter", "tiktoken", "patch", "parse", "-f", NOWHERE],
    ["validate", "--candidates"],
    ["validate", "--repo", NOWHERE],
    ["validate", "--candidates", "c", "--timeout", "x"],
    ["validate", "--candidates", "c", "--timeout", "1.5"],
    ["validate", "--candidates", "c", "--jobs", ""],
    ["validate", "--candidates", "c", "--run-timeout", "1e3"],
    ["validate", "--candidates", "c", "--repeat-baseline"],
    ["validate", "--candidates", "c", "--pass-env", "A=B"],
    ["validate", "--candidates", "c", "--pass-env", "1A"],
    ["validate", "--candidates", "c", "--pass-env", ""],
    ["validate", "--candidates", "c", "--pass-env", "A\n"],
    ["validate", "--candidates", "c", "--pass-env", "OK", "--pass-env", "no-dash"],
    ["validate", "--candidates", "c", "-o"],
    ["validate", "--cand", "c", "--x"],
    ["validate", "--allow", "--candidates", "c"],
    ["validate", "--candidates", "c", "--timeout", "0", "--repo", NOWHERE],
    ["validate", "--candidates", "c", "--test-cmd", "true", "-oout", "--repo", NOWHERE],
    ["validate", "--candidates", "c", "--json"],
    ["vote", "--verdicts"],
    ["vote", "--json"],
    ["vote", "--verdicts", NOWHERE],
    ["vote", "--verdicts", NOWHERE, "--json"],
    ["vote", "--verdicts", "a", "--verdicts", f"{NOWHERE}/b/"],
    ["vote", "--verdicts", NOWHERE, "--repo", "r"],
    ["lint", "--candidates", NOWHERE, "--repo", NOWHERE],
    ["lint", "--diff", NOWHERE, "--only", "arity", "--repo", NOWHERE],
]


def branchy(name: str, cc: int) -> str:
    """A function of cyclomatic complexity ``cc``."""
    lines = [f"def {name}(value):"]
    for index in range(cc - 1):
        lines += [f"    if value == {index}:", f"        return {index}"]
    lines.append("    return -1")
    return "\n".join(lines) + "\n"


SOURCE = "def quote(sku):\n    return 1\n\n\nclass PriceBook:\n    def cost_of(self, sku):\n"
SOURCE += "        return quote(sku)\n"
REVIEWED = [
    ["init", "repo"],
    ["write", "repo/app.py", branchy("handle", 10)],
    ["write", "repo/pkg/__init__.py", ""],
    ["commit", "repo"],
    ["write", "repo/app.py", branchy("handle", 11)],
]
HOOK_FORM = [
    ["init", "repo"],
    ["write", "repo/core.py", SOURCE],
    ["commit", "repo"],
    ["write", "after.py", SOURCE + "\n\n" + branchy("fresh", 12)],
]
PLAIN = [["write", "plain/a.py", "x = 1\n"], ["write", "after.py", "x = 2\n"]]
DEEP = "/".join(f"d{level}" for level in range(1, 22))
# The clone index walk warns about a name that is not UTF-8, then refuses a path at depth 21.
WALK_REFUSED = [
    ["init", "repo"],
    ["write", "repo/a.py", "def f(x):\n    return x\n"],
    ["commit", "repo"],
    ["write", "repo/a.py", "def f(x):\n    if x:\n        return 1\n    return x\n"],
    ["latin", "repo/bad\xff.py", "z = 1\n"],
    ["write", f"repo/{DEEP}/x.txt", "x\n"],
]
ROOT = "<SANDBOX>/repo"
SINCE = ["lint", "--since", "HEAD"]

SCENARIOS: list[dict[str, object]] = [
    {"name": "since_json", "steps": REVIEWED, "cwd": "repo", "argv": [*SINCE, "--json"]},
    {"name": "since_text", "steps": REVIEWED, "cwd": "repo", "argv": SINCE},
    {"name": "since_from_subdir", "steps": REVIEWED, "cwd": "repo/pkg", "argv": SINCE},
    {"name": "since_repo_flag", "steps": REVIEWED, "cwd": "", "argv": [*SINCE, "--repo", ROOT]},
    {
        "name": "since_unknown_revision",
        "steps": REVIEWED,
        "cwd": "repo",
        "argv": ["lint", "--since", "no-such-branch"],
    },
    {
        "name": "fail_on_warning",
        "steps": REVIEWED,
        "cwd": "repo",
        "argv": [*SINCE, "--fail-on", "warning"],
    },
    {
        "name": "fail_on_not_checked",
        "steps": REVIEWED,
        "cwd": "repo",
        "argv": [*SINCE, "--fail-on", "not_checked"],
    },
    {
        "name": "fail_on_advisory",
        "steps": REVIEWED,
        "cwd": "repo",
        "argv": [*SINCE, "--fail-on", "advisory"],
    },
    {
        "name": "since_all_pair_checks",
        "steps": REVIEWED,
        "cwd": "repo",
        "argv": [*SINCE, "--only", "complexity_delta,contract_drift,clone_delta", "--json"],
    },
    {
        "name": "since_clone_delta",
        "steps": [*REVIEWED, ["write", "repo/copy.py", branchy("again", 11)]],
        "cwd": "repo",
        "argv": [*SINCE, "--only", "clone_delta", "--json"],
    },
    {"name": "capabilities_text", "steps": REVIEWED, "cwd": "repo", "argv": ["capabilities"]},
    {
        "name": "capabilities_json",
        "steps": REVIEWED,
        "cwd": "repo",
        "argv": ["capabilities", "--json", "--no-cache"],
    },
    {
        "name": "hook_form_json",
        "steps": HOOK_FORM,
        "cwd": "",
        "argv": [
            "lint",
            "--path",
            "core.py",
            "--after",
            "<SANDBOX>/after.py",
            "--json",
            "--repo",
            ROOT,
        ],
    },
    {
        "name": "hook_form_text",
        "steps": HOOK_FORM,
        "cwd": "repo",
        "argv": ["lint", "--path", "core.py", "--after", "../after.py"],
    },
    {
        "name": "outside_git",
        "steps": PLAIN,
        "cwd": "plain",
        "argv": ["lint", "--path", "a.py", "--after", "<SANDBOX>/after.py"],
    },
    {"name": "capabilities_outside_git", "steps": PLAIN, "cwd": "plain", "argv": ["capabilities"]},
    {
        "name": "since_walk_refused_after_a_warning",
        "steps": WALK_REFUSED,
        "cwd": "repo",
        "argv": [*SINCE, "--only", "clone_delta", "--json"],
    },
]


def block(path: str, search: str, replace: str) -> str:
    """One SEARCH/REPLACE block."""
    return f"### {path}\n<<<<<<< SEARCH\n{search}\n=======\n{replace}\n>>>>>>> REPLACE\n"


CALLER = "from core import quote\n\n\ndef run(items):\n    return [quote(item) for item in items]\n"
PATCH = "```python\n" + block("core.py", "    return 1", "    return 2") + "```\n"
EDITS_JSON = (
    '{"edits": [{"index": 0, "path": "core.py", "search": "    return 1",'
    ' "replace": "    return 3"}]}\n'
)
Scenario = dict[str, object]
PATCH_REPO = [
    ["init", "repo"],
    ["write", "repo/core.py", SOURCE],
    ["write", "repo/caller.py", CALLER],
    ["write", "repo/notes.rst", "alpha\nbravo\n"],
    ["commit", "repo"],
]


def patched(text: str) -> list[list[str]]:
    """The patch repository with ``text`` in patch.txt beside it."""
    return [*PATCH_REPO, ["write", "patch.txt", text]]


def patch_scenario(name: str, steps: list[list[str]], argv: list[str], **extra: str) -> Scenario:
    """A patch scenario that runs from the sandbox with --repo repo."""
    return {"name": name, "steps": steps, "cwd": "", "argv": [*argv, "--repo", "repo"], **extra}


CHECK = ["patch", "check", "-f", "patch.txt"]
APPLY = ["patch", "apply", "-f", "patch.txt"]
NORMALIZE = ["patch", "normalize", "-f", "patch.txt"]
NOT_FOUND = block("core.py", "    return NOTHING_LIKE_THIS", "    return 2")
PATCH_SCENARIOS: list[Scenario] = [
    {
        "name": "parse_file",
        "steps": patched(PATCH),
        "cwd": "",
        "argv": ["patch", "parse", "-f", "patch.txt"],
    },
    {"name": "parse_stdin", "steps": [], "cwd": "", "argv": ["patch", "parse"], "stdin": PATCH},
    {
        "name": "parse_dash",
        "steps": [],
        "cwd": "",
        "argv": ["patch", "parse", "-f", "-"],
        "stdin": PATCH,
    },
    {
        "name": "parse_stdin_keeps_carriage_returns",
        "steps": [],
        "cwd": "",
        "argv": ["patch", "parse"],
        "stdin": PATCH.replace("\n", "\r\n"),
    },
    {
        "name": "parse_file_reads_carriage_returns_as_line_breaks",
        "steps": patched(PATCH.replace("\n", "\r\n")),
        "cwd": "",
        "argv": ["patch", "parse", "-f", "patch.txt"],
    },
    {
        "name": "parse_malformed",
        "steps": [],
        "cwd": "",
        "argv": ["patch", "parse"],
        "stdin": "### a.py\n<<<<<<< SEARCH\nx\n>>>>>>> REPLACE\n" + block("", "y", "z"),
    },
    {
        "name": "parse_edits_json",
        "steps": [],
        "cwd": "",
        "argv": ["patch", "parse"],
        "stdin": EDITS_JSON,
    },
    {"name": "parse_empty", "steps": [], "cwd": "", "argv": ["patch", "parse"], "stdin": ""},
    {"name": "parse_directory", "steps": [], "cwd": "", "argv": ["patch", "parse", "-f", "."]},
    {"name": "parse_missing", "steps": [], "cwd": "", "argv": ["patch", "parse", "-f", "gone.txt"]},
    {
        "name": "check_text",
        "steps": patched(PATCH),
        "cwd": "repo",
        "argv": ["patch", "check", "-f", "../patch.txt"],
    },
    patch_scenario("check_json", patched(PATCH), [*CHECK, "--json"]),
    patch_scenario(
        "check_broken", patched(block("core.py", "    return 1", "    return (")), CHECK
    ),
    patch_scenario("check_not_found", patched(NOT_FOUND), CHECK),
    patch_scenario("check_not_found_json", patched(NOT_FOUND), [*CHECK, "--json"]),
    patch_scenario(
        "check_escape", patched(PATCH.replace("### core.py", "### ../escape.py")), CHECK
    ),
    patch_scenario("check_missing_file", patched(block("absent.py", "x", "y")), [*CHECK, "--json"]),
    patch_scenario(
        "check_no_grammar", patched(block("notes.rst", "bravo", "BRAVO")), [*CHECK, "--json"]
    ),
    patch_scenario(
        "check_truncated", patched(PATCH + "### caller.py\n<<<<<<< SEARCH\nfrom core\n"), CHECK
    ),
    patch_scenario("check_edits_json", patched(EDITS_JSON), CHECK),
    patch_scenario("check_stdin", PATCH_REPO, ["patch", "check"], stdin=PATCH),
    patch_scenario("check_unreadable_patch", PATCH_REPO, ["patch", "check", "-f", "gone.txt"]),
    {
        "name": "check_outside_git",
        "steps": [["write", "plain/core.py", SOURCE], ["write", "patch.txt", PATCH]],
        "cwd": "plain",
        "argv": ["patch", "check", "-f", "../patch.txt"],
    },
    patch_scenario("apply_text", patched(PATCH), APPLY),
    patch_scenario("apply_json", patched(PATCH), [*APPLY, "--json"]),
    patch_scenario("apply_in_place", patched(PATCH), [*APPLY, "--in-place"]),
    patch_scenario(
        "apply_in_place_dirty",
        [*patched(PATCH), ["write", "repo/scratch.txt", "wip\n"]],
        [*APPLY, "--in-place"],
    ),
    patch_scenario("apply_not_found", patched(NOT_FOUND), APPLY),
    patch_scenario(
        "apply_no_change", patched(block("core.py", "    return 1", "    return 1")), APPLY
    ),
    patch_scenario(
        "apply_two_files",
        patched(PATCH + block("caller.py", "def run(items):", "def run_all(items):")),
        [*APPLY, "--json"],
    ),
    patch_scenario("normalize_text", patched(PATCH), NORMALIZE),
    patch_scenario("normalize_json", patched(PATCH), [*NORMALIZE, "--json"]),
    patch_scenario("normalize_not_found", patched(NOT_FOUND), [*NORMALIZE, "--json"]),
    patch_scenario(
        "normalize_no_grammar", patched(block("notes.rst", "bravo", "BRAVO")), NORMALIZE
    ),
]

# The test commands of the validate scenarios: a regression check that passes until `add` goes,
# and a reproduction check that fails until the sign is fixed.
REGRESSION = 'grep -q "def add" app.py || { echo "add is gone" >&2; echo checked; exit 1; }\n'
REPRODUCTION = 'grep -q "a + b" app.py\n'
SEEDED = [
    ["init", "repo"],
    ["write", "repo/app.py", "def add(a, b):\n    return a - b\n"],
    ["write", "repo/conftest.py", "collect_ignore = []\n"],
    ["write", "repo/check_regression.sh", REGRESSION],
    ["write", "repo/check_repro.sh", REPRODUCTION],
    ["commit", "repo"],
]


def sign_fix(replace: str) -> str:
    """A candidate that rewrites the seeded `return a - b`."""
    return block("app.py", "    return a - b", f"    {replace}")


CANDIDATE_FILES = {
    "01-plus.txt": sign_fix("return a + b"),
    "02-plus-commented.txt": sign_fix("return a  +  b  # restore the sign"),
    "03-swapped.txt": sign_fix("return b + a"),
    "04-times.txt": sign_fix("return a * b"),
    "05-renames.txt": block("app.py", "def add(a, b):", "def plus(a, b):"),
    "06-misses.txt": sign_fix("return a + b").replace("a - b", "a - c"),
    "07-conftest.txt": block("conftest.py", "collect_ignore = []", 'collect_ignore = ["app.py"]'),
}


def candidates(*names: str) -> list[list[str]]:
    """The seeded repository with the named candidate files in cands/."""
    return [*SEEDED, *(["write", f"cands/{name}", CANDIDATE_FILES[name]] for name in names)]


EVERY_CANDIDATE = candidates(*CANDIDATE_FILES)
VALIDATE = [
    "validate",
    "--candidates",
    "cands",
    "--repo",
    "repo",
    "--test-cmd",
    "sh check_regression.sh",
]
REPRO = ["--repro-cmd", "sh check_repro.sh"]
RED = [*SEEDED[:-1], ["write", "repo/check_regression.sh", "exit 1\n"], ["commit", "repo"]]
CONFIGURED = [
    *SEEDED[:-1],
    ["write", "repo/.agentless-mcp.json", '{"test_cmd": "sh check_regression.sh"}\n'],
    ["commit", "repo"],
    ["write", "cands/01-plus.txt", CANDIDATE_FILES["01-plus.txt"]],
]
FROM_CONFIG = ["validate", "--candidates", "cands", "--repo", "repo"]
VALIDATE_SCENARIOS: list[Scenario] = [
    {"name": "validate_stdout", "steps": EVERY_CANDIDATE, "cwd": "", "argv": [*VALIDATE, *REPRO]},
    {
        "name": "validate_output",
        "steps": EVERY_CANDIDATE,
        "cwd": "",
        "argv": [*VALIDATE, "-o", "out.jsonl"],
    },
    {
        "name": "validate_output_unwritable",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": [*VALIDATE, "-o", "nodir//out.jsonl"],
    },
    {
        "name": "validate_red_baseline",
        "steps": [*RED, ["write", "cands/01-plus.txt", CANDIDATE_FILES["01-plus.txt"]]],
        "cwd": "",
        "argv": [*VALIDATE, *REPRO],
    },
    {
        "name": "validate_repro_does_not_reproduce",
        "steps": candidates("01-plus.txt", "04-times.txt"),
        "cwd": "",
        "argv": [*VALIDATE, "--repro-cmd", "sh check_regression.sh"],
    },
    {
        "name": "validate_nothing_passes",
        "steps": candidates("05-renames.txt", "06-misses.txt"),
        "cwd": "",
        "argv": VALIDATE,
    },
    {
        "name": "validate_jobs_two",
        "steps": EVERY_CANDIDATE,
        "cwd": "",
        "argv": [*VALIDATE, *REPRO, "--jobs", "2"],
    },
    {
        "name": "validate_repeat_baseline",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": [*VALIDATE, "--repeat-baseline", "2"],
    },
    {
        "name": "validate_test_config_edit_allowed",
        "steps": candidates("07-conftest.txt"),
        "cwd": "",
        "argv": [*VALIDATE, "--allow-test-config-edits"],
    },
    {
        "name": "validate_from_the_git_root",
        "steps": candidates("01-plus.txt"),
        "cwd": "repo",
        "argv": ["validate", "--candidates", "../cands", "--test-cmd", "sh check_regression.sh"],
    },
    {
        "name": "validate_timeout_zero",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": [*VALIDATE, "--timeout", "0"],
    },
    {
        "name": "validate_jobs_zero",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": [*VALIDATE, "--jobs", "-1"],
    },
    {
        "name": "validate_jobs_over",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": [*VALIDATE, "--jobs", "201"],
    },
    {
        "name": "validate_repeat_zero",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": [*VALIDATE, "--repeat-baseline", "0"],
    },
    {
        "name": "validate_run_timeout_zero",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": [*VALIDATE, "--run-timeout", "0"],
    },
    {
        "name": "validate_pass_env_over",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": [*VALIDATE, *[word for _ in range(33) for word in ("--pass-env", "HOME")]],
    },
    {
        "name": "validate_pass_env_repeated",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": [*VALIDATE, *[word for _ in range(32) for word in ("--pass-env", "HOME")]],
    },
    {
        "name": "validate_no_test_cmd",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": FROM_CONFIG,
    },
    {
        "name": "validate_config_test_cmd_refused",
        "steps": CONFIGURED,
        "cwd": "",
        "argv": FROM_CONFIG,
    },
    {
        "name": "validate_config_test_cmd_allowed",
        "steps": CONFIGURED,
        "cwd": "",
        "argv": [*FROM_CONFIG, "--allow-repo-test-cmd"],
    },
    {
        "name": "validate_outside_git",
        "steps": [["write", "plain/app.py", "x = 1\n"]],
        "cwd": "plain",
        "argv": ["validate", "--candidates", "c", "--test-cmd", "true"],
    },
    {
        "name": "validate_not_a_commit",
        "steps": [["write", "plain/app.py", "x = 1\n"], ["write", "cands/01.txt", sign_fix("x")]],
        "cwd": "",
        "argv": ["validate", "--candidates", "cands", "--repo", "plain", "--test-cmd", "true"],
    },
    {"name": "validate_missing_candidates", "steps": SEEDED, "cwd": "", "argv": VALIDATE},
    {
        "name": "validate_empty_candidates",
        "steps": [*SEEDED, ["write", "cands/.keep", ""]],
        "cwd": "",
        "argv": VALIDATE,
    },
    {
        "name": "validate_unsplittable_command",
        "steps": candidates("01-plus.txt"),
        "cwd": "",
        "argv": [*VALIDATE[:-1], "sh 'check_regression.sh"],
    },
    {
        "name": "validate_nothing_applies",
        "steps": candidates("06-misses.txt"),
        "cwd": "",
        "argv": VALIDATE,
    },
]

# The validate scenarios whose verdicts the vote scenarios read, by the name of each document.
VOTED = {
    "reproduced": "validate_stdout",
    "not_reproduced": "validate_repro_does_not_reproduce",
    "unverified": "validate_red_baseline",
    "nothing_applied": "validate_nothing_applies",
}


def verdicts_documents() -> dict[str, str]:
    """Each VOTED document as the reference's validate wrote it, with every duration 0.0 and the
    sandbox as /srv, so that vote reads files of the reference's own form."""
    named = {str(scenario["name"]): scenario for scenario in VALIDATE_SCENARIOS}
    with ThreadPoolExecutor(max_workers=4) as pool:
        runs = pool.map(scenario_case, (named[name] for name in VOTED.values()))
        rows = dict(zip(VOTED, runs, strict=True))
    fixed = '"duration": 0.0'
    return {
        name: DURATION.sub(fixed, str(row["out"])).replace("<SANDBOX>", "/srv")
        for name, row in rows.items()
    }


def vote_scenarios(documents: dict[str, str]) -> list[Scenario]:
    """vote over each document, in both forms, and over the files it refuses."""
    scenarios: list[Scenario] = []
    for name, document in documents.items():
        steps = [["write", "verdicts.jsonl", document]]
        for form in ([], ["--json"]):
            argv = ["vote", "--verdicts", "verdicts.jsonl", *form]
            label = f"vote_{name}{'_json' if form else ''}"
            scenarios.append({"name": label, "steps": steps, "cwd": "", "argv": argv})
    refused = {
        "vote_empty": [["write", "verdicts.jsonl", ""]],
        "vote_malformed": [["write", "verdicts.jsonl", "{not json}\n"]],
        "vote_no_run_record": [["write", "verdicts.jsonl", '{"record": "candidate"}\n']],
        "vote_missing": [],
        "vote_directory": [["write", "verdicts.jsonl/x", ""]],
    }
    for label, steps in refused.items():
        argv = ["vote", "--verdicts", "verdicts.jsonl"]
        scenarios.append({"name": label, "steps": steps, "cwd": "", "argv": argv})
    return scenarios


# lint's candidate and diff forms over the patch repository: a rename that leaves callers, a call
# with an argument too many, and the same rename as a unified diff.
DIFF = (
    "diff --git a/core.py b/core.py\n--- a/core.py\n+++ b/core.py\n@@ -1,2 +1,2 @@\n"
    "-def quote(sku):\n+def price(sku):\n     return 1\n"
)
LINTED = [
    *PATCH_REPO,
    ["write", "cands/01-renames.txt", block("core.py", "def quote(sku):", "def price(sku):")],
    [
        "write",
        "cands/02-calls.txt",
        block(
            "caller.py", "    return [quote(item) for item in items]", "    return [quote(item, 2)]"
        ),
    ],
    ["write", "change.diff", DIFF],
    ["write", "notes.txt", "no patch here\n"],
]
CANDIDATES = ["lint", "--candidates", "cands", "--repo", "repo"]
LINT_DIFF = ["lint", "--diff", "change.diff", "--repo", "repo"]
LINT_SCENARIOS: list[Scenario] = [
    {
        "name": "lint_candidates_text",
        "steps": LINTED,
        "cwd": "repo",
        "argv": ["lint", "--candidates", "../cands"],
    },
    {"name": "lint_candidates_json", "steps": LINTED, "cwd": "", "argv": [*CANDIDATES, "--json"]},
    {
        "name": "lint_candidates_one_file",
        "steps": LINTED,
        "cwd": "",
        "argv": ["lint", "--candidates", "cands/01-renames.txt", "--repo", "repo"],
    },
    {
        "name": "lint_candidates_only",
        "steps": LINTED,
        "cwd": "",
        "argv": [*CANDIDATES, "--only", "arity,dangling_callers", "--json"],
    },
    {
        "name": "lint_candidates_only_pairs",
        "steps": LINTED,
        "cwd": "",
        "argv": [*CANDIDATES, "--only", "complexity_delta"],
    },
    {
        "name": "lint_candidates_fail_on",
        "steps": LINTED,
        "cwd": "",
        "argv": [*CANDIDATES, "--fail-on", "warning"],
    },
    {
        "name": "lint_candidates_unparsed",
        "steps": LINTED,
        "cwd": "",
        "argv": ["lint", "--candidates", "notes.txt", "--repo", "repo"],
    },
    {
        "name": "lint_candidates_missing",
        "steps": LINTED,
        "cwd": "",
        "argv": ["lint", "--candidates", "gone", "--repo", "repo"],
    },
    {"name": "lint_diff_text", "steps": LINTED, "cwd": "", "argv": LINT_DIFF},
    {"name": "lint_diff_json", "steps": LINTED, "cwd": "", "argv": [*LINT_DIFF, "--json"]},
    {
        "name": "lint_diff_not_a_diff",
        "steps": LINTED,
        "cwd": "",
        "argv": ["lint", "--diff", "notes.txt", "--repo", "repo"],
    },
    {
        "name": "lint_diff_missing",
        "steps": LINTED,
        "cwd": "",
        "argv": ["lint", "--diff", "gone.diff", "--repo", "repo"],
    },
]

# The DURATION mask: a verdict record's run time is the one free value of the write scenarios,
# so each "duration" value is this placeholder. The cli test applies the same mask.
DURATION = re.compile(r'"duration": -?[0-9][0-9.e+-]*')
DURATION_MASK = '"duration": <DURATION>'


def environment(sandbox: Path, columns: str | None) -> dict[str, str]:
    """The whole environment of one run: nothing of the caller's reaches it but PATH."""
    env = {
        **hermetic_env(sandbox / "home"),
        GRAMMAR_OVERRIDE: os.environ[GRAMMAR_OVERRIDE],
        "TIKTOKEN_CACHE_DIR": os.environ["TIKTOKEN_CACHE_DIR"],
        "AGENTLESS_MCP_NO_AUTO_WARM": "1",
        "AGENTLESS_MCP_NO_DOWNLOAD": "1",
        "LANG": "C.UTF-8",
    }
    if columns is not None:
        env["COLUMNS"] = columns
    return env


def run_cli(
    argv: list[str], cwd: Path, env: dict[str, str], stdin: str | None = None
) -> dict[str, object]:
    """Run the console script's entry point once; return its stdout, stderr and exit code.

    ``stdin`` is the text on its stdin; ``None`` leaves it the caller's.
    """
    command = [sys.executable, "-I", "-c", BOOT, *argv]
    given = None if stdin is None else stdin.encode("utf-8")
    done = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        input=given,
        capture_output=True,
        timeout=TIMEOUT_SECONDS,
        check=False,
    )
    return {
        "out": done.stdout.decode("utf-8"),
        "err": done.stderr.decode("utf-8"),
        "code": done.returncode,
    }


def parser_case(argv: list[str], columns: str | None) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="cli-fixture-") as made:
        sandbox = Path(made).resolve()
        (sandbox / "home").mkdir()
        result = run_cli(argv, sandbox, environment(sandbox, columns))
    for stream in ("out", "err"):
        if str(sandbox) in str(result[stream]):
            sys.exit(f"refused: the output of {argv} names its sandbox; make it a scenario")
    return {"argv": argv, "columns": columns, **result}


def parser_fixture() -> Fixture:
    cases = [(argv, width) for argv in HELP_ARGVS for width in WIDTHS]
    cases += [(["--help"], columns) for columns in ODD_COLUMNS]
    cases += [(argv, None) for argv in CORPUS]
    with ThreadPoolExecutor(max_workers=8) as pool:
        rows = list(pool.map(lambda case: parser_case(*case), cases))
    return {"runs": rows}


def apply(sandbox: Path, env: dict[str, str], step: list[str]) -> None:
    """Run one scenario step in ``sandbox``. The Rust test runs the same steps."""
    kind, place, *rest = step
    target = sandbox / place
    if kind == "write":
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(rest[0], encoding="utf-8")
    elif kind == "latin":
        # The file name is the latin-1 bytes of `place`, so a character above U+007F is no UTF-8.
        named = Path(os.fsdecode(os.fsencode(sandbox) + b"/" + place.encode("latin-1")))
        named.parent.mkdir(parents=True, exist_ok=True)
        named.write_text(rest[0], encoding="utf-8")
    elif kind == "init":
        target.mkdir(parents=True, exist_ok=True)
        run_git(target, IDENTITY, "init", "--quiet", "-b", "main", env=env)
    elif kind == "commit":
        run_git(target, IDENTITY, "add", "-A", env=env)
        message = rest[0] if rest else "fixture"
        run_git(target, IDENTITY, "commit", "--quiet", "-m", message, env=env)
    elif kind == "index":
        done = run_cli(["index"], target, env)
        if done["code"] != 0:
            sys.exit(f"refused: the index step in {place} exited {done['code']}: {done['err']}")
    else:
        sys.exit(f"refused: no scenario step {kind}")


def normalised(text: str, sandbox: Path) -> str:
    """Replace what depends on the sandbox, the build or the grammar copy, by rule."""
    text = text.replace(str(sandbox), "<SANDBOX>")
    text = re.sub(r"(<SANDBOX>/home/\.cache/agentless-mcp/)[^/\s\"]+/", r"\1<KEY>/", text)
    return "\n".join(normal_line(line) for line in text.split("\n"))


def normal_line(line: str) -> str:
    """The version and the grammar cache of the capabilities answer, as placeholders."""
    stripped = line.lstrip(" ")
    indent = line[: len(line) - len(stripped)]
    comma = "," if line.endswith(",") else ""
    if line == f"agentless-mcp {VERSION}":
        return "agentless-mcp <VERSION>"
    if line.startswith("pack ") and "  grammar cache " in line:
        return line.partition("  grammar cache ")[0] + "  grammar cache <GRAMMAR_CACHE>"
    if stripped.startswith('"version": "'):
        return f'{indent}"version": "<VERSION>"{comma}'
    if stripped.startswith('"grammar_cache": "'):
        return f'{indent}"grammar_cache": "<GRAMMAR_CACHE>"{comma}'
    return line


def scenario_case(scenario: dict[str, object]) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="cli-fixture-") as made:
        sandbox = Path(made).resolve()
        (sandbox / "home").mkdir()
        env = environment(sandbox, None)
        steps = scenario["steps"]
        assert isinstance(steps, list)
        for step in steps:
            apply(sandbox, env, step)
        raw_argv = scenario["argv"]
        assert isinstance(raw_argv, list)
        argv = [str(word).replace("<SANDBOX>", str(sandbox)) for word in raw_argv]
        stdin = scenario.get("stdin")
        assert stdin is None or isinstance(stdin, str)
        result = run_cli(argv, sandbox / str(scenario["cwd"]), env, stdin)
        for stream in ("out", "err"):
            result[stream] = normalised(str(result[stream]), sandbox)
    return {**scenario, **result}


def scenario_fixture() -> Fixture:
    with ThreadPoolExecutor(max_workers=8) as pool:
        rows = list(pool.map(scenario_case, SCENARIOS))
    return {"runs": rows}


def help_texts() -> list[str]:
    """Every description and help string of the top, lint and capabilities parsers."""
    top = build_parser()
    commands = subparsers_of(top)
    parsers = [top, commands.choices["lint"], commands.choices["capabilities"]]
    texts = [str(parser.description) for parser in parsers if parser.description]
    for parser in parsers:
        texts += [str(action.help) for action in parser._actions if action.help]
    texts += [str(choice.help) for choice in commands._choices_actions]
    return list(dict.fromkeys(texts))


def write_help_texts() -> list[str]:
    """Every help string of the write parsers: patch and its steps, validate and vote."""
    commands = subparsers_of(build_parser())
    steps = subparsers_of(commands.choices["patch"])
    parsers = [
        commands.choices["patch"],
        *steps.choices.values(),
        commands.choices["validate"],
        commands.choices["vote"],
    ]
    texts = [str(action.help) for parser in parsers for action in parser._actions if action.help]
    return texts + [str(choice.help) for choice in steps._choices_actions]


def subparsers_of(
    parser: argparse.ArgumentParser,
) -> "argparse._SubParsersAction[argparse.ArgumentParser]":
    """The subcommands action of ``parser``."""
    return next(a for a in parser._actions if isinstance(a, argparse._SubParsersAction))


def slices(text: str, lines: list[str]) -> list[list[int]]:
    """Each wrapped line as [start, length]: a substring of ``text`` after the line before it."""
    found = []
    position = 0
    for line in lines:
        start = text.index(line, position)
        found.append([start, len(line)])
        position = start + len(line)
    return found


def wrap_case(text: str) -> dict[str, object]:
    collapsed = re.sub(r"\s+", " ", text, flags=re.ASCII).strip()
    runs: list[list[object]] = []
    for width in range(10, 201):
        lines = slices(collapsed, textwrap.wrap(collapsed, width))
        if runs and runs[-1][2] == lines:
            runs[-1][1] = width
        else:
            runs.append([width, width, lines])
    return {"text": text, "collapsed": collapsed, "runs": runs}


def wrap_fixture() -> Fixture:
    # The write parsers' texts come last, so the earlier cases keep their places.
    texts = dict.fromkeys([*help_texts(), *WRAP_EXTRAS, *write_help_texts()])
    return {"cases": [wrap_case(text) for text in texts]}


def write_scenario_row(scenario: Scenario) -> dict[str, object]:
    """One write scenario's answer, with the DURATION mask applied."""
    row = scenario_case(scenario)
    for stream in ("out", "err"):
        row[stream] = DURATION.sub(DURATION_MASK, str(row[stream]))
    return row


def write_scenario_fixture() -> Fixture:
    """patch, validate, vote and lint's patch forms end to end over sandbox repositories."""
    scenarios = [
        *PATCH_SCENARIOS,
        *VALIDATE_SCENARIOS,
        *vote_scenarios(verdicts_documents()),
        *LINT_SCENARIOS,
    ]
    with ThreadPoolExecutor(max_workers=8) as pool:
        rows = list(pool.map(write_scenario_row, scenarios))
    return {"runs": rows}


BUILDERS: dict[str, Callable[[], Fixture]] = {
    "parser": parser_fixture,
    "scenarios": scenario_fixture,
    "wrap": wrap_fixture,
    "write_scenarios": write_scenario_fixture,
}


if __name__ == "__main__":
    refuse_real_caches()
    if not os.environ.get(GRAMMAR_OVERRIDE):
        sys.exit(f"refused: set {GRAMMAR_OVERRIDE} to a scratch copy of the grammar libraries")
    # The Rust build links every grammar, so a cold grammar here would pin a different answer.
    cold = set(grammars.ALL_LANGUAGES) - grammars.warmed_languages()
    if cold:
        sys.exit(f"refused: {GRAMMAR_OVERRIDE} holds no library for {', '.join(sorted(cold))}")
    raise SystemExit(write_or_check((__doc__ or "").splitlines()[0], FIXTURES, BUILDERS))
