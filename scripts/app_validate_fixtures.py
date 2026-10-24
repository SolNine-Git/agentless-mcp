"""Write the app crate's validate fixtures: each input with the result Python gives.

Development-time only. Run it from the repository root, with the grammar libraries copied to a
scratch directory, so that no run reads or writes the user's grammar cache:

    TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy> \
        uv run python scripts/app_validate_fixtures.py

With --check, the script compares its output with the committed fixtures and writes nothing.

The service cases run the validate service over a git repository with a scripted test runner:
each run returns the next scripted result and moves the clock by its advance, and the clock moves
only then. The receipt of the run record is replaced by "<RECEIPT>", which the Rust test passes.
"""

import json
import logging
import os
import re
import sys
import tempfile
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any
from unittest import mock

from app_fixtures import apply, isolated, repo
from fixture_files import Fixture, write_or_check

from agentless_mcp.application import validate_service
from agentless_mcp.application.patch_service import PatchService
from agentless_mcp.application.repo_context import resolve_repo
from agentless_mcp.core import vote
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.core.sandbox import RunResult, RunStatus
from agentless_mcp.util.errors import AgentlessError

FIXTURES = Path(__file__).resolve().parents[1] / "crates" / "app" / "fixtures" / "validate"
GRAMMAR_OVERRIDE = "TREE_SITTER_LANGUAGE_PACK_CACHE_DIR"
RECEIPT = "<RECEIPT>"
SANDBOX = "<SANDBOX>"
BOM = chr(0xFEFF)
LINE_SEPARATOR = chr(0x2028)
PRIVATE_USE = chr(0xE000)


def jsonl(*records: dict[str, object]) -> str:
    """A verdicts document of ``records``, one per line."""
    return "".join(json.dumps(record) + "\n" for record in records)


HEADER = {
    "record": "run",
    "baseline": "ok",
    "repro_valid": True,
    "test_cmd": "pytest -q",
    "repro_cmd": "pytest -q tests/test_bug.py",
}


def candidate(name: str, index: int, **fields: object) -> dict[str, object]:
    """One candidate record that the reader accepts, with ``fields`` changed."""
    record: dict[str, object] = {
        "record": "candidate",
        "id": name,
        "index": index,
        "apply": {"status": "ok", "reasons": []},
        "regression": "passed",
        "reproduction": "passed",
        "equivalence_key": f"key-{name}",
    }
    record.update(fields)
    return record


def without(record: dict[str, object], name: str) -> dict[str, object]:
    """``record`` without the field ``name``."""
    return {key: value for key, value in record.items() if key != name}


GOOD = [
    candidate("c1", 0),
    candidate("c2", 1, equivalence_key="key-c1", reproduction="failed"),
    candidate("c3", 2, regression="failed", reproduction=None),
    candidate("c4", 3, regression="timeout", reproduction="not_evaluated"),
    candidate("c5", 4, regression="error", reproduction=None),
    candidate("c6", 5, apply={"status": "failed", "reasons": ["x"]}, equivalence_key=None),
    candidate("c7", 6, regression="not_evaluated", reproduction=None),
    candidate("c8", 7, apply={"status": "not_evaluated", "reasons": []}),
]

VERDICT_TEXTS = [
    "",
    "\n  \n\t\n",
    jsonl(HEADER, *GOOD),
    jsonl({**HEADER, "repro_valid": False}, *GOOD),
    jsonl({**HEADER, "baseline": "unverified", "repro_cmd": None}),
    jsonl(HEADER),
    "\n" + jsonl(HEADER, GOOD[0]).replace("\n", "\r\n\r\n") + "   \n",
    jsonl(HEADER) + "not json\n",
    jsonl(HEADER) + "[1, 2]\n",
    jsonl(HEADER) + '{"record": "candidate",}\n',
    BOM + jsonl(HEADER),
    jsonl(without(HEADER, "record")),
    jsonl({**HEADER, "record": "candidate"}),
    jsonl({**HEADER, "record": 1}),
    jsonl(without(HEADER, "repro_valid"), GOOD[0]),
    jsonl({**HEADER, "repro_valid": 1}),
    jsonl({**HEADER, "baseline": "green"}),
    jsonl(without(HEADER, "baseline")),
    jsonl({**HEADER, "test_cmd": 5}),
    jsonl(without(HEADER, "repro_cmd")),
    jsonl({**HEADER, "repro_cmd": ["a"]}),
    jsonl({**HEADER, "baseline": "green"}, {"record": "summary"}),
    jsonl(HEADER, GOOD[0], HEADER),
    jsonl(HEADER, {"record": "summary"}),
    jsonl(HEADER, {"record": "it's"}),
    jsonl(HEADER, {"record": None}),
    jsonl(HEADER, without(GOOD[0], "apply")),
    jsonl(HEADER, {**GOOD[0], "apply": ["ok"]}),
    jsonl(HEADER, {**GOOD[0], "equivalence_key": 5}),
    jsonl(HEADER, without(GOOD[0], "equivalence_key")),
    jsonl(HEADER, without(GOOD[0], "regression")),
    jsonl(HEADER, {**GOOD[0], "regression": "passing"}),
    jsonl(HEADER, without(GOOD[0], "reproduction")),
    jsonl(HEADER, {**GOOD[0], "reproduction": "nope"}),
    jsonl(HEADER, without(GOOD[0], "id")),
    jsonl(HEADER, {**GOOD[0], "id": 7}),
    jsonl(HEADER, without(GOOD[0], "index")),
    jsonl(HEADER, {**GOOD[0], "index": "1"}),
    jsonl(HEADER, {**GOOD[0], "index": True}),
    jsonl(HEADER, {**GOOD[0], "index": 1.0}),
    jsonl(HEADER, {**GOOD[0], "index": -3}, {**GOOD[1], "index": 9223372036854775807}),
    jsonl(HEADER, {**GOOD[0], "apply": {"reasons": []}}),
    jsonl(HEADER, {**GOOD[0], "apply": {"status": "maybe"}}),
    jsonl(HEADER, GOOD[0], {**GOOD[1], "id": "c1"}),
    jsonl(HEADER, GOOD[0], {**GOOD[1], "index": 0}),
    jsonl(HEADER, candidate("\xe9t\xe9", 0), candidate("\xe9t\xe9", 1)),
    jsonl(HEADER, {**GOOD[0], "extra": {"nested": [1]}}),
    (
        '{"record": "run", "baseline": "bad", "baseline": "ok", "repro_valid": true, '
        '"test_cmd": "t", "repro_cmd": null}\n'
    ),
    jsonl(HEADER) + '{"record": "candidate", "id": "a' + LINE_SEPARATOR + 'b"}\n',
    jsonl(HEADER) + '{"record": "candidate", "id": "a\\u2028b"}\n',
]


def loaded_row(text: str) -> list[object]:
    """The document that ``text`` reads to with its vote, or the refusal text."""
    try:
        loaded = validate_service.load_verdicts(text)
    except AgentlessError as refused:
        return [text, {"error": str(refused)}]
    candidates = [
        [
            entry.id,
            entry.index,
            entry.applied,
            entry.measured,
            entry.equivalence_key,
            entry.regression_passed,
            entry.reproduction_passed,
        ]
        for entry in loaded.candidates
    ]
    document = {
        "baseline": loaded.baseline.value,
        "repro_valid": loaded.repro_valid,
        "test_cmd": loaded.test_cmd,
        "repro_cmd": loaded.repro_cmd,
        "candidates": candidates,
    }
    ranked = vote.rank(loaded.candidates, repro_valid=loaded.repro_valid)
    return [text, {"loaded": document, "vote": json.dumps(ranked.as_dict())}]


def verdicts_fixture() -> Fixture:
    """Every load_verdicts case, and the vote over each document that reads."""
    return {"texts": [loaded_row(text) for text in VERDICT_TEXTS]}


def files(directory: str, names: dict[str, str]) -> list[list[object]]:
    """The steps that write ``names`` below ``directory``."""
    return [["mkdir", directory], *(["write", f"{directory}/{n}", t] for n, t in names.items())]


LOADS: list[dict[str, Any]] = [
    {
        "name": "sorted_stems",
        "steps": files(
            "cands",
            {"b.patch": "B\n", "a.txt": "A\n", "c": "C\n", ".hidden": "H\n", "x.tar.gz": "X\n"},
        ),
        "target": "cands",
    },
    {"name": "empty", "steps": [["mkdir", "cands"]], "target": "cands"},
    {"name": "missing", "steps": [], "target": "missing"},
    {"name": "a_file", "steps": [["write", "one.txt", "x\n"]], "target": "one.txt"},
    {
        "name": "shared_stem",
        "steps": files("cands", {"a.txt": "1\n", "a.patch": "2\n", "0.txt": "0\n"}),
        "target": "cands",
    },
    {
        "name": "subdirectory_and_links",
        "steps": [
            *files("cands", {"a.txt": "A\n"}),
            ["mkdir", "cands/sub"],
            ["write", "elsewhere.txt", "E\n"],
            ["symlink", "cands/linked.txt", "<SANDBOX>/elsewhere.txt"],
            ["symlink", "cands/broken.txt", "<SANDBOX>/nowhere.txt"],
        ],
        "target": "cands",
    },
    {
        "name": "too_many",
        "steps": files("cands", {f"c{index:03d}.txt": "x\n" for index in range(201)}),
        "target": "cands",
    },
    {
        "name": "too_large",
        "steps": [*files("cands", {"a.txt": "A\n"}), ["repeat", "cands/big.txt", "x", 1_000_001]],
        "target": "cands",
    },
    {
        "name": "not_utf8",
        "steps": [*files("cands", {"a.txt": "A\n"}), ["bytes", "cands/bad.txt", "x\xff\n"]],
        "target": "cands",
    },
    {
        "name": "line_ends",
        "steps": files(
            "cands", {"crlf.txt": "a\r\nb\r\n", "cr.txt": "a\rb\r\r\n", "bom": BOM + "x\n"}
        ),
        "target": "cands",
    },
    {"name": "home", "steps": files("home/cands", {"a.txt": "A\n"}), "target": "~/cands"},
    {"name": "relative", "steps": files("cands", {"a.txt": "A\n"}), "target": "./cands/../cands"},
    {
        "name": "code_point_order",
        "steps": [
            *files("cands", {"a.txt": "A\n", PRIVATE_USE + ".txt": "P\n"}),
            ["latin_name", "cands/\xff.txt", "F\n"],
        ],
        "target": "cands",
    },
    {
        "name": "unreadable",
        "steps": [*files("cands", {"a.txt": "A\n", "b.txt": "B\n"}), ["chmod", "cands/b.txt", 0]],
        "target": "cands",
    },
]


def refusal(refused: AgentlessError) -> list[str]:
    """The kind and the text of a refusal; the kind is the class name without "Error"."""
    return [type(refused).__name__.removesuffix("Error"), str(refused)]


def _modes(steps: list[list[Any]], sandbox: Path, restore: bool) -> None:
    for _, place, mode in (step for step in steps if step[0] == "chmod"):
        (sandbox / str(place)).chmod(0o644 if restore else int(mode))


def _shown(text: str, sandbox: Path) -> str:
    # The approved non-UTF-8 rule: the Rust build names a byte that is not UTF-8 with U+FFFD.
    # A worktree's name is random, so the Rust test masks it the same way.
    text = re.sub(r"\\udc[89a-f][0-9a-f]", r"\\ufffd", text.replace(str(sandbox), SANDBOX))
    return re.sub(r"wt-[0-9a-f]{32}", "wt-<ID>", text)


# A scratch sandbox with the steps applied, the chmod steps last, and the sandbox as the cwd.
@contextmanager
def _scenario(steps: list[list[Any]]) -> Iterator[Path]:
    with tempfile.TemporaryDirectory(prefix="app-validate-") as made:
        sandbox = Path(made).resolve()
        cwd = Path.cwd()
        with isolated(sandbox):
            for step in steps:
                if step[0] != "chmod":
                    apply(sandbox, step)
            _modes(steps, sandbox, restore=False)
            os.chdir(sandbox)
            try:
                yield sandbox
            finally:
                os.chdir(cwd)
                _modes(steps, sandbox, restore=True)


def load_row(case: dict[str, Any]) -> dict[str, Any]:
    """Load one candidate layout and record the candidates or the refusal."""
    with _scenario(case["steps"]) as sandbox:
        try:
            loaded = validate_service.load_candidates(Path(str(case["target"])))
            expected: dict[str, object] = {
                "candidates": [[entry.id, entry.index, entry.text] for entry in loaded]
            }
        except AgentlessError as refused:
            expected = {"error": refusal(refused)}
        shown = _shown(json.dumps(expected), sandbox)
    return {**case, "expected": json.loads(shown)}


FINGERPRINTS = [
    [],
    [["a.py", "x = 1\n"]],
    [["a.py", "\xe9t\xe9\n"], ["b/c.py", '"quoted" \\ back\t\x01\x7f\n']],
    [["z.py", LINE_SEPARATOR + " \U0001f600 </script>"]],
]

TEST_CONFIG_PATHS = [
    ["conftest.py", "tests/unit/conftest.py", "app.py"],
    ["./.github/workflows/ci.yml", "src/../.github/x", ".github", "github/x"],
    ["Makefile", "makefile.txt", "pkg/setup.py", "a/../setup.cfg", "GNUmakefile"],
    ["//.github/x", "/.github/x", "../conftest.py", "setup.py/", "x/../../tox.ini"],
    ["pyproject.toml", ".pre-commit-config.yaml", "noxfile.py", "pytest.ini", "package.json"],
    ["sitecustomize.py", "usercustomize.py", "makefile", "conftest.py", "conftest.py"],
    ["src/app.py", "docs/conf.py", ".github"],
    [],
]


def inputs_fixture() -> Fixture:
    """The candidate loader, the execution fingerprint and the test-config guard."""
    guarded = []
    for paths in TEST_CONFIG_PATHS:
        named = validate_service.test_config_paths(paths)
        reason = validate_service._test_config_reasons(named)[0] if named else None
        guarded.append([paths, list(named), reason])
    fingerprints = [
        [key, validate_service._execution_fingerprint(tuple((p, c) for p, c in key))]
        for key in FINGERPRINTS
    ]
    return {
        "candidates": [load_row(case) for case in LOADS],
        "fingerprints": fingerprints,
        "test_config": guarded,
    }


APP = "def add(a, b):\n    return a - b\n"


def block(path: str, search: str, replace: str) -> str:
    """One SEARCH/REPLACE block for ``path``."""
    return f"### {path}\n<<<<<<< SEARCH\n{search}=======\n{replace}>>>>>>> REPLACE\n"


PLUS = block("app.py", "    return a - b\n", "    return a + b\n")
PLUS_EDIT = {"path": "./app.py", "search": "    return a - b\n", "replace": "    return a + b\n"}
PLUS_JSON = json.dumps({"edits": [PLUS_EDIT]})
SPACED = block("app.py", "    return a - b\n", "    return a+b\n")
WRONG = block("app.py", "    return a - b\n", "    return a * b\n")
MISSING = block("app.py", "    return nothing\n", "    return a + b\n")
NOTHING = "no blocks here\n"
UNTERMINATED = "### app.py\n<<<<<<< SEARCH\n    return a - b\n"
BAD_JSON = '{"edits": [}'
CONFTEST = block("tests/conftest.py", "X = 1\n", "X = 2\n")
ESCAPE = block("../outside.py", "secret\n", "leaked\n")
SAME = block("app.py", "    return a - b\n", "    return a - b\n")

BASE = repo({"app.py": APP, "tests/conftest.py": "X = 1\n"})
NESTED = repo({"sub/app.py": APP, "top.py": "Y = 1\n"})
FOUR = files(
    "cands",
    {"01-plus.txt": PLUS, "02-json.json": PLUS_JSON, "03-spaced": SPACED, "04-wrong.txt": WRONG},
)
TWO = files("cands", {"01-plus.txt": PLUS, "02-wrong.txt": WRONG})

PASS = ["passed", 0, 1.25, "ok\n", ""]
FAIL = ["failed", 1, 0.5, "", "FAILED test_add\n"]
TIMEOUT = ["timeout", None, 60.0, "partial\n", ""]
ERROR = ["error", None, 0.0, "", "could not start 'nope': No such file or directory"]
CRASH = ["failed", -11, 0.125, "", "Segmentation fault\n"]


def runs(*scripted: tuple[list[object], float]) -> list[list[object]]:
    """Each run's result and its clock advance."""
    return [[*result, advance] for result, advance in scripted]


def service_case(
    name: str,
    steps: list[list[object]],
    scripted: list[list[object]],
    **request: object,
) -> dict[str, Any]:
    """One validate case: the steps, the request fields that differ, and the scripted runs."""
    fields: dict[str, object] = {
        "candidates": "cands",
        "test_cmd": "run-tests",
        "repro_cmd": None,
        "timeout": 300,
        "jobs": 1,
        "repeat_baseline": 1,
        "run_timeout": None,
        "pass_env": [],
        "test_cmd_from_repo": False,
        "allow_repo_test_cmd": False,
        "allow_test_config_edits": False,
    }
    root = str(request.pop("root", "repo"))
    fields.update(request)
    return {"name": name, "steps": steps, "root": root, "request": fields, "runs": scripted}


REPRO = {"repro_cmd": "run-repro"}

SERVICES = [
    service_case(
        "four_candidates",
        [*BASE, *FOUR],
        runs((PASS, 1.0), (PASS, 2.0), (PASS, 0.5), (FAIL, 0.25)),
    ),
    service_case("red_baseline", [*BASE, *TWO], runs((FAIL, 1.0))),
    service_case("baseline_timeout", [*BASE, *TWO], runs((TIMEOUT, 60.0)), timeout=60),
    service_case("baseline_error", [*BASE, *TWO], runs((ERROR, 0.0))),
    service_case("baseline_signal", [*BASE, *TWO], runs((CRASH, 0.0))),
    service_case(
        "repro_does_not_reproduce",
        [*BASE, *TWO],
        runs((PASS, 1.0), (PASS, 1.0), (PASS, 1.0), (FAIL, 1.0)),
        **REPRO,
    ),
    service_case(
        "repro_reproduces",
        [*BASE, *TWO],
        runs((PASS, 1.0), (FAIL, 1.0), (PASS, 1.0), (PASS, 0.5), (FAIL, 1.0)),
        **REPRO,
    ),
    service_case(
        "repro_fails_after_the_fix",
        [*BASE, *TWO],
        runs((PASS, 1.0), (TIMEOUT, 1.0), (PASS, 1.0), (FAIL, 0.5), (FAIL, 1.0)),
        **REPRO,
    ),
    service_case(
        "repro_unrunnable",
        [*BASE, *TWO],
        runs((PASS, 1.0), (ERROR, 0.0), (PASS, 1.0), (FAIL, 1.0)),
        **REPRO,
    ),
    service_case(
        "flaky_baseline",
        [*BASE, *TWO],
        runs((PASS, 1.0), (FAIL, 1.0), (PASS, 1.0)),
        repeat_baseline=3,
    ),
    service_case("red_twice", [*BASE, *TWO], runs((FAIL, 1.0), (FAIL, 1.0)), repeat_baseline=2),
    service_case(
        "green_twice",
        [*BASE, *TWO],
        runs((PASS, 1.0), (PASS, 1.0), (PASS, 1.0), (FAIL, 1.0)),
        repeat_baseline=2,
    ),
    service_case(
        "budget_cuts_the_baseline",
        [*BASE, *TWO],
        runs((TIMEOUT, 2.0)),
        timeout=60,
        run_timeout=2,
    ),
    service_case(
        "budget_spent_on_the_baseline",
        [*BASE, *FOUR],
        runs((PASS, 20.0)),
        run_timeout=10,
    ),
    service_case(
        "budget_spent_before_the_command",
        [*BASE, *TWO],
        runs((PASS, 9.5)),
        run_timeout=10,
    ),
    service_case(
        "budget_clamps_a_command",
        [*BASE, *TWO],
        runs((PASS, 50.0), (TIMEOUT, 50.0)),
        timeout=60,
        run_timeout=100,
    ),
    service_case(
        "budget_spent_before_the_repro_baseline",
        [*BASE, *TWO],
        runs((PASS, 9.5)),
        run_timeout=10,
        **REPRO,
    ),
    service_case(
        "budget_spent_before_the_repro",
        [*BASE, *TWO],
        runs((PASS, 1.0), (FAIL, 1.0), (PASS, 7.5)),
        run_timeout=10,
        **REPRO,
    ),
    service_case(
        "budget_spent_between_repeats",
        [*BASE, *TWO],
        runs((PASS, 10.0)),
        repeat_baseline=2,
        run_timeout=10,
    ),
    service_case(
        "budget_nobody_spends",
        [*BASE, *TWO],
        runs((PASS, 1.0), (PASS, 1.0), (FAIL, 1.0)),
        run_timeout=10000,
    ),
    service_case(
        "never_measured",
        [*BASE, *TWO],
        runs((PASS, 1.0), (ERROR, 0.0), (PASS, 1.0)),
    ),
    service_case(
        "unowned_command",
        [*BASE, *TWO],
        [],
        test_cmd="pytest -k 'add'",
        test_cmd_from_repo=True,
    ),
    service_case(
        "unowned_command_opted_in",
        [*BASE, *TWO],
        runs((FAIL, 1.0)),
        test_cmd_from_repo=True,
        allow_repo_test_cmd=True,
    ),
    service_case("jobs_zero", [*BASE, *TWO], [], jobs=0),
    service_case("jobs_above_the_cap", [*BASE, *TWO], [], jobs=201),
    service_case("timeout_zero", [*BASE, *TWO], [], timeout=0),
    service_case("repeat_zero", [*BASE, *TWO], [], repeat_baseline=0),
    service_case("run_timeout_zero", [*BASE, *TWO], [], run_timeout=0),
    service_case("refusal_order", [*BASE], [], jobs=0, test_cmd_from_repo=True),
    service_case("bounds_before_candidates", [*BASE], [], timeout=-5),
    service_case("missing_candidates", [*BASE], []),
    service_case(
        "no_commit",
        [["mkdir", "repo"], ["write", "repo/app.py", APP], ["init", "repo"], *TWO],
        [],
    ),
    service_case(
        "test_config_refused",
        [*BASE, *files("cands", {"01-conf.txt": CONFTEST, "02-plus.txt": PLUS})],
        runs((PASS, 1.0), (PASS, 1.0)),
    ),
    service_case(
        "test_config_allowed",
        [*BASE, *files("cands", {"01-conf.txt": CONFTEST, "02-plus.txt": PLUS})],
        runs((PASS, 1.0), (PASS, 1.0), (PASS, 1.0)),
        allow_test_config_edits=True,
    ),
    service_case(
        "escape_refuses_the_run",
        [*BASE, *files("cands", {"01-plus.txt": PLUS, "02-escape.txt": ESCAPE})],
        runs((PASS, 1.0)),
    ),
    service_case(
        "unusable_candidates",
        [
            *BASE,
            *files(
                "cands",
                {
                    "01-missing.txt": MISSING,
                    "02-nothing.txt": NOTHING,
                    "03-unterminated.txt": UNTERMINATED,
                    "04-bad.json": BAD_JSON,
                    "05-plus.txt": PLUS,
                    "06-same.txt": SAME,
                },
            ),
        ],
        runs((PASS, 1.0), (PASS, 1.0), (PASS, 1.0)),
    ),
    service_case(
        "nothing_to_run",
        [*BASE, *files("cands", {"01-missing.txt": MISSING, "02-nothing.txt": NOTHING})],
        runs((PASS, 1.0)),
    ),
    service_case(
        "subdirectory_root",
        [*NESTED, *TWO],
        runs((PASS, 1.0), (PASS, 1.0), (FAIL, 1.0)),
        root="repo/sub",
    ),
    service_case(
        "two_jobs",
        [*BASE, *FOUR],
        runs((PASS, 0.0), (PASS, 0.0), (PASS, 0.0), (PASS, 0.0)),
        jobs=2,
    ),
]


class _Warnings(logging.Handler):
    def __init__(self) -> None:
        super().__init__(logging.WARNING)
        self.lines: list[str] = []

    def emit(self, record: logging.LogRecord) -> None:
        self.lines.append(record.getMessage())


def _request(sandbox: Path, fields: dict[str, Any]) -> validate_service.ValidateRequest:
    return validate_service.ValidateRequest(
        candidates=sandbox / str(fields["candidates"]),
        test_cmd=str(fields["test_cmd"]),
        repro_cmd=fields["repro_cmd"],
        timeout=int(fields["timeout"]),
        jobs=int(fields["jobs"]),
        repeat_baseline=int(fields["repeat_baseline"]),
        run_timeout=None if fields["run_timeout"] is None else int(fields["run_timeout"]),
        passthrough_env=tuple(str(name) for name in fields["pass_env"]),
        test_cmd_from_repo=bool(fields["test_cmd_from_repo"]),
        allow_repo_test_cmd=bool(fields["allow_repo_test_cmd"]),
        allow_test_config_edits=bool(fields["allow_test_config_edits"]),
    )


def _receipted(document: str) -> str:
    records = [json.loads(line) for line in document.splitlines()]
    for record in records:
        if record["record"] == "run":
            record["receipt"] = RECEIPT
    return "".join(json.dumps(record) + "\n" for record in records)


class _Runner:
    def __init__(self, name: str, scripted: list[list[Any]]) -> None:
        self.name = name
        self.queue = list(scripted)
        self.now = 0.0
        self.calls: list[list[object]] = []

    def clock(self) -> float:
        return self.now

    def run(self, cwd: Path, cmd: str, *, timeout: float, passthrough_env: object) -> RunResult:
        _ = passthrough_env
        probe = Path(cwd) / "app.py"
        text = probe.read_text(encoding="utf-8") if probe.is_file() else None
        self.calls.append([cmd, timeout, text])
        if not self.queue:
            sys.exit(f"{self.name}: call {len(self.calls)} has no scripted run")
        status, code, duration, stdout, stderr, advance = self.queue.pop(0)
        self.now += float(advance)
        return RunResult(RunStatus(status), code, float(duration), str(stdout), str(stderr))


def _validated(sandbox: Path, case: dict[str, Any]) -> dict[str, object]:
    service = validate_service.ValidateService(PatchService(TreeSitterExtractor()))
    ctx = resolve_repo(sandbox / str(case["root"]), None)
    try:
        report = service.validate(ctx, _request(sandbox, case["request"]))
    except AgentlessError as refused:
        return {"error": refusal(refused)}
    return {
        "jsonl": _receipted(report.jsonl()),
        "summary": report.summary_line(),
        "warnings": list(report.warnings()),
        "any_passed": report.any_passed,
    }


def service_row(case: dict[str, Any]) -> dict[str, Any]:
    """Run one validate case and record the report or the refusal, the calls, and the log."""
    with _scenario(case["steps"]) as sandbox:
        runner = _Runner(str(case["name"]), case["runs"])
        warnings = _Warnings()
        logging.getLogger().addHandler(warnings)
        clock = mock.patch.object(validate_service, "_monotonic", runner.clock)
        commands = mock.patch.object(validate_service.sandbox, "run_command", runner.run)
        try:
            with clock, commands:
                expected = _validated(sandbox, case)
        finally:
            logging.getLogger().removeHandler(warnings)
        if runner.queue:
            sys.exit(f"{case['name']}: {len(runner.queue)} scripted runs were not used")
        calls = runner.calls if case["request"]["jobs"] == 1 else sorted(runner.calls, key=str)
        shown = _shown(json.dumps([expected, calls, warnings.lines]), sandbox)
    expected, calls, lines = json.loads(shown)
    return {**case, "expected": expected, "calls": calls, "diagnostics": lines}


def service_fixture() -> Fixture:
    """Every validate case, each in its own sandbox."""
    if not os.environ.get(GRAMMAR_OVERRIDE):
        sys.exit(f"refused: set {GRAMMAR_OVERRIDE} to a scratch copy of the grammar libraries")
    os.environ["AGENTLESS_MCP_NO_DOWNLOAD"] = "1"
    os.environ["AGENTLESS_MCP_NO_AUTO_WARM"] = "1"
    return {"cases": [service_row(case) for case in SERVICES]}


BUILDERS: dict[str, Callable[[], Fixture]] = {
    "verdicts": verdicts_fixture,
    "inputs": inputs_fixture,
    "service": service_fixture,
}


if __name__ == "__main__":
    raise SystemExit(write_or_check((__doc__ or "").splitlines()[0], FIXTURES, BUILDERS))
