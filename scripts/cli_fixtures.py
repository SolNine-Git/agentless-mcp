"""Write the cli crate's test fixtures: what the Python command line prints for each command line.

Development-time only. Run it from the repository root, with the grammar libraries copied to a
scratch directory, so that no run reads or writes the user's grammar cache:

    TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy>/v1.14.3/libs \
        uv run python scripts/cli_fixtures.py

With --check, the script compares its output with the committed fixtures and writes nothing.

Each case runs the console script's entry point in a fresh interpreter, with HOME, the caches and
the git config inside a temporary sandbox. A blocked import of tiktoken stands for an install
without the 'tokens' extra.
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
import sys

class _NoTiktoken:
    def find_spec(self, name, path=None, target=None):
        if name == "tiktoken" or name.startswith("tiktoken."):
            raise ModuleNotFoundError(f"No module named {name!r}", name=name)
        return None

sys.meta_path.insert(0, _NoTiktoken())
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
    ["--token-counter", "tiktoken", "capabilities"],
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
    ["--token-counter", "chars4", "--token-counter", "tiktoken", "capabilities"],
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


def environment(sandbox: Path, columns: str | None) -> dict[str, str]:
    """The whole environment of one run: nothing of the caller's reaches it but PATH."""
    env = {
        **hermetic_env(sandbox / "home"),
        GRAMMAR_OVERRIDE: os.environ[GRAMMAR_OVERRIDE],
        "AGENTLESS_MCP_NO_AUTO_WARM": "1",
        "AGENTLESS_MCP_NO_DOWNLOAD": "1",
        "LANG": "C.UTF-8",
    }
    if columns is not None:
        env["COLUMNS"] = columns
    return env


def run_cli(argv: list[str], cwd: Path, env: dict[str, str]) -> dict[str, object]:
    """Run the console script's entry point once; return its stdout, stderr and exit code."""
    command = [sys.executable, "-I", "-c", BOOT, *argv]
    done = subprocess.run(
        command, cwd=cwd, env=env, capture_output=True, timeout=TIMEOUT_SECONDS, check=False
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
        run_git(target, IDENTITY, "commit", "--quiet", "-m", "fixture", env=env)
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
        result = run_cli(argv, sandbox / str(scenario["cwd"]), env)
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
    commands = next(a for a in top._actions if isinstance(a, argparse._SubParsersAction))
    parsers = [top, commands.choices["lint"], commands.choices["capabilities"]]
    texts = [str(parser.description) for parser in parsers if parser.description]
    for parser in parsers:
        texts += [str(action.help) for action in parser._actions if action.help]
    texts += [str(choice.help) for choice in commands._choices_actions]
    return list(dict.fromkeys(texts))


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
    return {"cases": [wrap_case(text) for text in [*help_texts(), *WRAP_EXTRAS]]}


BUILDERS: dict[str, Callable[[], Fixture]] = {
    "parser": parser_fixture,
    "scenarios": scenario_fixture,
    "wrap": wrap_fixture,
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
