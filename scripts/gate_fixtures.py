"""Write the search gate fixtures: each payload with what the reference gate hooks do with it.

Development-time only. Run it from the repository root with the project environment:

    uv run python scripts/gate_fixtures.py

Each case runs the reference gate-check and gate-mark hooks in process, in its own temporary
directory, which the fixtures write as <SANDBOX>. A step whose stderr is a pipe with no reader
runs the hook as a process, because the interpreter's flush at exit sets that exit code. The hooks
keep their markers in <SANDBOX>/markers, so no case reads or writes the real marker directory.
The clock is pinned to the nanosecond count of clock.json, and a probe file name shows the
process id as <PID>. Paths, file contents and stdin are stored as latin-1 text of their bytes.
With --check, the script compares its output with the committed fixtures and writes nothing.
"""

import contextlib
import hashlib
import importlib.util
import io
import json
import os
import re
import subprocess
import sys
import tempfile
from collections.abc import Callable, Iterator
from dataclasses import dataclass, field
from pathlib import Path
from types import ModuleType, SimpleNamespace
from typing import Any

from fixture_files import Fixture, write_or_check

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates" / "hook" / "fixtures" / "gate"
HOOKS = ROOT / "contrib" / "hooks"
TESTS_PATH = ROOT / "tests" / "unit" / "test_gate_hooks.py"
PLACEHOLDER = "<SANDBOX>"
PID_MASK = "<PID>"
MARKERS = "markers"
LOG = "gate.jsonl"
# Not whole seconds, so the logged ts has a fraction.
WALL_NS = 1_760_000_123_456_789_123
SAFE_PATH = re.compile(r"[A-Za-z0-9/._-]+")
# The deepest document that json.loads reads.
MAX_DEPTH = 9998
INT_DIGITS = 4300
LONG_NAME = "n" * 300
TOOL_PREFIX = "mcp__agentless__"
NAIVE = "na" + chr(0xEF) + "ve"
LINE_SEPARATOR = chr(0x2028)
GRINNING = chr(0x1F600)
PROCESS_SECONDS = 60
# Loads one hook with the given marker directory and clock, and exits with the code of its main(),
# so the interpreter's flush of stderr at exit can still change the exit code.
PROCESS_HOOK = """
import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location("hook", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.MARKER_DIR = Path(sys.argv[2])
seconds = float(sys.argv[3])
module.time = SimpleNamespace(time=lambda: seconds)
raise SystemExit(module.main())
"""

Json = Any
Op = list[Any]


def load(path: Path, name: str) -> ModuleType:
    """Load a script as a module without putting its directory on sys.path."""
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        message = f"refused: {path} does not load as a module"
        raise SystemExit(message)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# The cases set the hooks' marker directory and clock, which a ModuleType does not declare.
check: Any = load(HOOKS / "agentless_gate_check.py", "agentless_gate_check")
mark: Any = load(HOOKS / "agentless_gate_mark.py", "agentless_gate_mark")
reference_tests = load(TESTS_PATH, "test_gate_hooks")


def pinned_time() -> float:
    # time.time divides a nanosecond count that is not whole seconds as a float.
    return float(WALL_NS) / 1e9


for _hook in (check, mark):
    _hook.time = SimpleNamespace(time=pinned_time)


def latin(data: bytes) -> str:
    """Return bytes as the latin-1 text that the fixtures store them as."""
    return data.decode("latin-1")


def parametrized(function: Callable[..., object]) -> list[Any]:
    """Return the argument values of the one parametrize mark on a reference test."""
    marks = [item for item in getattr(function, "pytestmark", []) if item.name == "parametrize"]
    if len(marks) != 1:
        message = f"refused: {function.__name__} has {len(marks)} parametrize marks"
        raise SystemExit(message)
    return list(marks[0].args[1])


def node(function: Callable[..., object], part: str = "") -> str:
    """Return the pytest node name of one reference test or one of its parameter sets."""
    return f"{function.__name__}[{part}]" if part else function.__name__


@dataclass
class Step:
    """One hook run: the payload on stdin, the process cwd, the log variable, and the stderr."""

    hook: str
    stdin: bytes
    # Below the sandbox; None runs in a directory that was removed.
    cwd: str | None = ""
    # Below the sandbox; None leaves the variable unset, and "" sets it empty.
    log: str | None = LOG
    stderr_fails: bool = False


@dataclass
class Case:
    """Setup operations below the sandbox, then steps in order in that one sandbox."""

    name: str
    steps: list[Step]
    setup: list[Op] = field(default_factory=list)


def as_stdin(value: Json) -> bytes:
    """Return raw stdin as given, or a payload as json.dumps writes it."""
    return value if isinstance(value, bytes) else json.dumps(value).encode("ascii")


def checked(value: Json, **options: Any) -> Step:
    return Step("check", as_stdin(value), **options)


def marked(value: Json, **options: Any) -> Step:
    return Step("mark", as_stdin(value), **options)


def native(session: str, tool: str, **tool_input: Json) -> dict[str, Json]:
    return {"session_id": session, "cwd": PLACEHOLDER, "tool_name": tool, "tool_input": tool_input}


def shell(command: Json, session: str = "shell", tool: str = "Bash") -> dict[str, Json]:
    return {
        "session_id": session,
        "cwd": PLACEHOLDER,
        "tool_name": tool,
        "tool_input": {"command": command},
    }


def raw_shell(command: bytes) -> bytes:
    """Return a Bash payload whose command is JSON string text, escapes included."""
    head = b'{"session_id": "shell", "cwd": "' + PLACEHOLDER.encode() + b'", "tool_name": "Bash", '
    return head + b'"tool_input": {"command": "' + command + b'"}}'


def raw_grep(path: bytes, cwd: bytes = PLACEHOLDER.encode()) -> bytes:
    """Return a Grep payload whose path and cwd are JSON string text, escapes included."""
    head = b'{"session_id": "exact", "cwd": "' + cwd + b'", "tool_name": "Grep", '
    return head + b'"tool_input": {"pattern": "x", "path": "' + path + b'"}}'


def structural(session: str, tool: str, **tool_input: Json) -> dict[str, Json]:
    return {"session_id": session, "tool_name": TOOL_PREFIX + tool, "tool_input": tool_input}


def broad(session: str) -> dict[str, Json]:
    return {"session_id": session, "tool_name": "Grep", "tool_input": {"pattern": "VALUE"}}


def marker_name(session: str) -> str:
    return hashlib.sha256(session.encode()).hexdigest() + ".json"


class Sandbox:
    """One case's temporary directory, with its paths written as the placeholder."""

    def __init__(self) -> None:
        self.path = Path(tempfile.mkdtemp(prefix="gate-")).resolve()
        if not SAFE_PATH.fullmatch(str(self.path)):
            message = f"refused: {self.path} needs JSON escapes; set TMPDIR to a plain path"
            raise SystemExit(message)

    def placed(self, text: str) -> str:
        return text.replace(PLACEHOLDER, str(self.path)).replace(PID_MASK, str(os.getpid()))

    def masked(self, text: str) -> str:
        return text.replace(str(self.path), PLACEHOLDER).replace(
            f".write-probe-{os.getpid()}", f".write-probe-{PID_MASK}"
        )

    def at(self, relative: str) -> Path:
        """Return the path below the sandbox that the latin-1 text of its bytes names."""
        return self.path / os.fsdecode(self.placed(relative).encode("latin-1"))

    def apply(self, op: Op) -> None:
        kind, relative, *rest = op
        target = self.at(relative)
        if kind == "dir":
            target.mkdir(parents=True)
        elif kind in ("file", "marker"):
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(rest[0].encode("latin-1"))
        elif kind == "link":
            target.symlink_to(rest[0])
        elif kind == "mode":
            target.chmod(rest[0])
        else:
            message = f"refused: unknown setup operation {kind}"
            raise SystemExit(message)

    def close(self) -> None:
        opened(self.path)
        for path in sorted(self.path.rglob("*"), reverse=True):
            if path.is_dir() and not path.is_symlink():
                path.rmdir()
            else:
                path.unlink()
        self.path.rmdir()


def opened(directory: Path) -> None:
    """Give each directory below and at ``directory`` its owner's full mode, parents first."""
    directory.chmod(0o700)
    for child in directory.iterdir():
        if child.is_dir() and not child.is_symlink():
            opened(child)


def log_variable(box: Sandbox, step: Step) -> str | None:
    """Return the step's AGENTLESS_GATE_LOG value, or None to leave the variable unset."""
    if step.log is None:
        return None
    return str(box.path / step.log) if step.log else ""


@contextlib.contextmanager
def process(box: Sandbox, step: Step, stderr: io.StringIO) -> Iterator[None]:
    """Run the body with the step's cwd, log variable, stdin and stderr in place."""
    raw = box.placed(latin(step.stdin)).encode("latin-1")
    stdin = io.TextIOWrapper(io.BytesIO(raw), encoding="utf-8", errors="strict", newline="\n")
    saved = (sys.stdin, sys.stderr, os.environ.get(check.LOG_ENV), Path.cwd())
    variable = log_variable(box, step)
    if variable is None:
        os.environ.pop(check.LOG_ENV, None)
    else:
        os.environ[check.LOG_ENV] = variable
    if step.cwd is None:
        gone = box.path / "gone"
        gone.mkdir()
        os.chdir(gone)
        gone.rmdir()
    else:
        os.chdir(box.path / step.cwd)
    sys.stdin, sys.stderr = stdin, stderr
    try:
        yield
    finally:
        sys.stdin, sys.stderr, logged, start = saved
        os.chdir(start)
        if logged is None:
            os.environ.pop(check.LOG_ENV, None)
        else:
            os.environ[check.LOG_ENV] = logged


def listing(box: Sandbox) -> list[list[str | None]] | None:
    """Return the marker directory's entries and file bytes, or None when it is not listable."""
    try:
        children = sorted((box.path / MARKERS).iterdir(), key=lambda child: os.fsencode(child.name))
    except OSError:
        return None
    entries: list[list[str | None]] = []
    for child in children:
        try:
            content: str | None = latin(child.read_bytes())
        except OSError:
            content = None
        entries.append([box.masked(latin(os.fsencode(child.name))), content])
    return entries


def in_process(box: Sandbox, step: Step) -> tuple[int, str]:
    module = check if step.hook == "check" else mark
    stderr = io.StringIO()
    with process(box, step, stderr):
        code = module.main()
    return code, stderr.getvalue()


def closed_stderr(box: Sandbox, step: Step) -> tuple[int, str]:
    """Run the hook as a process whose stderr is a pipe with no reader, and return its code."""
    if step.cwd is None:
        message = "refused: a process step needs a cwd"
        raise SystemExit(message)
    name = "agentless_gate_check.py" if step.hook == "check" else "agentless_gate_mark.py"
    env = {key: value for key, value in os.environ.items() if key != check.LOG_ENV}
    variable = log_variable(box, step)
    if variable is not None:
        env[check.LOG_ENV] = variable
    hook = [str(HOOKS / name), str(box.path / MARKERS), repr(pinned_time())]
    read_end, write_end = os.pipe()
    os.close(read_end)
    try:
        done = subprocess.run(
            [sys.executable, "-I", "-c", PROCESS_HOOK, *hook],
            input=box.placed(latin(step.stdin)).encode("latin-1"),
            stdout=subprocess.DEVNULL,
            stderr=write_end,
            cwd=box.path / step.cwd,
            env=env,
            timeout=PROCESS_SECONDS,
            check=False,
        )
    finally:
        os.close(write_end)
    return done.returncode, ""


def run(box: Sandbox, step: Step) -> dict[str, Json]:
    code, stderr = closed_stderr(box, step) if step.stderr_fails else in_process(box, step)
    log_path = box.path / step.log if step.log else None
    log_text = latin(log_path.read_bytes()) if log_path is not None and log_path.is_file() else None
    return {
        "hook": step.hook,
        "stdin": latin(step.stdin),
        "cwd": step.cwd,
        "log": step.log,
        "stderr_fails": step.stderr_fails,
        "code": code,
        "stderr": stderr,
        "log_text": None if log_text is None else box.masked(log_text),
        "markers": listing(box),
    }


def outcome(case: Case) -> dict[str, Json]:
    box = Sandbox()
    try:
        check.MARKER_DIR = mark.MARKER_DIR = box.path / MARKERS
        for op in case.setup:
            box.apply(op)
        steps = [run(box, step) for step in case.steps]
    finally:
        box.close()
    return {
        "name": case.name,
        "needs_modes": any(op[0] == "mode" for op in case.setup),
        "setup": case.setup,
        "steps": steps,
    }


def reference_marker(session: str, tool: str, operation: str) -> Op:
    """Return a setup operation with the marker bytes that the reference mark hook writes."""
    box = Sandbox()
    try:
        mark.MARKER_DIR = box.path / MARKERS
        run(box, marked(structural(session, tool, operation=operation), log=None))
        path = box.path / MARKERS / marker_name(session)
        return ["marker", f"{MARKERS}/{path.name}", latin(path.read_bytes())]
    finally:
        box.close()


def tests_cases() -> list[Case]:
    """The 59 reference tests that run a hook, one case per test node."""
    t = reference_tests
    cases = [
        Case(
            node(t.test_an_exact_file_grep_needs_no_structural_marker),
            [checked(native("known-file", "Grep", pattern="VALUE", path="app.py"))],
            [["file", "app.py", "VALUE = 1\n"]],
        )
    ]
    function = t.test_broad_discovery_is_denied_before_localization
    for index, (tool, tool_input, _) in enumerate(parametrized(function)):
        steps = [checked(native("broad", tool, **tool_input))]
        cases.append(Case(node(function, f"case{index}"), steps, [["dir", "src"]]))
    function = t.test_localizing_operations_unlock_broad_search
    for index, (tool, tool_input, reason) in enumerate(parametrized(function)):
        steps = [
            marked({"session_id": "localized", "tool_name": tool, "tool_input": tool_input}),
            checked(broad("localized")),
        ]
        cases.append(Case(node(function, f"{tool}-tool_input{index}-{reason}"), steps))
    function = t.test_non_localizing_calls_do_not_unlock_broad_search
    for index, (tool, tool_input) in enumerate(parametrized(function)):
        steps = [
            marked({"session_id": "diagnostic", "tool_name": tool, "tool_input": tool_input}),
            checked(broad("diagnostic")),
        ]
        cases.append(Case(node(function, f"{tool}-tool_input{index}"), steps))
    cases += single_tests(t)
    for function, table, session, setup in (
        (
            t.test_a_bash_tree_search_is_denied_before_localization,
            t.BASH_DENIED,
            "bash-deny",
            [["dir", "src"]],
        ),
        (t.test_bash_commands_that_are_not_tree_searches_pass, t.BASH_ALLOWED, "bash-allow", []),
    ):
        for label, command in table:
            cases.append(Case(node(function, label), [checked(shell(command, session))], setup))
    cases += shell_tests(t)
    function = t.test_structurally_malformed_payloads_fail_open
    for index, value in enumerate(parametrized(function)):
        cases.append(Case(node(function, f"payload{index}"), [checked(value)]))
    cases += fail_open_tests(t)
    return cases


def single_tests(t: ModuleType) -> list[Case]:
    scoped = {
        "session_id": "sliced",
        "tool_name": "Grep",
        "tool_input": {"pattern": "VALUE", "path": f"{PLACEHOLDER}/app.py"},
    }
    slice_call = structural("sliced", "read", operation="slice", path="app.py", lines=[[1, 5]])
    return [
        Case(
            node(t.test_naming_a_file_and_a_range_unlocks_like_an_exact_file_grep),
            [
                checked(scoped),
                checked(broad("sliced")),
                marked(slice_call),
                checked(broad("sliced")),
            ],
            [["file", "app.py", "x = 1\n"]],
        ),
        Case(
            node(t.test_a_listing_is_how_you_look_not_proof_you_found),
            [marked(structural("listing", "read", operation="dir")), checked(broad("listing"))],
        ),
        Case(
            node(t.test_the_log_records_which_operation_was_refused),
            [
                marked(structural("logged", "read", operation="dir")),
                marked(structural("logged", "read", operation="slice")),
            ],
        ),
    ]


def shell_tests(t: ModuleType) -> list[Case]:
    unlock = [
        checked(shell("rg DEADLINE", "bash-unlock")),
        marked(structural("bash-unlock", "orient", operation="map")),
        checked(shell("rg DEADLINE", "bash-unlock")),
    ]
    cases = [
        Case(
            node(t.test_a_bash_search_scoped_to_one_file_passes),
            [checked(shell("rg DEADLINE app.py", "bash-file"))],
            [["file", "app.py", "DEADLINE = 1\n"]],
        ),
        Case(node(t.test_bash_search_unlocks_with_the_same_marker), unlock),
        Case(
            node(t.test_the_denial_names_the_shell_route_in_the_log),
            [checked(shell("rg DEADLINE", "bash-log"))],
        ),
    ]
    function = t.test_every_shell_tool_name_is_gated
    for tool in parametrized(function):
        steps = [checked(shell("rg DEADLINE", f"shell-{tool}", tool))]
        cases.append(Case(node(function, tool), steps))
    second = {"session_id": "second", "tool_name": "Glob", "tool_input": {"pattern": "**/*.py"}}
    cases.append(
        Case(
            node(t.test_sessions_do_not_unlock_each_other),
            [marked(structural("first", "orient", operation="map")), checked(second)],
        )
    )
    missing = {"tool_name": "Glob", "tool_input": {"pattern": "**/*.py"}}
    cases.append(
        Case(
            node(t.test_missing_session_and_malformed_input_fail_open),
            [checked(missing), checked(b"{")],
        )
    )
    return cases


def fail_open_tests(t: ModuleType) -> list[Case]:
    ungoverned = {"session_id": "wide", "tool_name": "Read", "tool_input": {"file_path": "a.py"}}
    unreadable = {"session_id": "wide", "tool_name": "Grep", "tool_input": {}}
    no_storage = {
        "session_id": "no-storage",
        "tool_name": "Glob",
        "tool_input": {"pattern": "**/*.py"},
    }
    return [
        Case(
            node(t.test_a_tool_this_gate_does_not_govern_is_named_as_such),
            [checked(ungoverned), checked(unreadable)],
        ),
        # The reference test replaces the probe. Here the marker directory is a plain file.
        Case(
            node(t.test_unavailable_marker_storage_fails_open),
            [checked(no_storage)],
            [["file", MARKERS, ""]],
        ),
    ]


SHELL_SHAPES = [
    "true || rg DEADLINE",
    "cd src ; rg DEADLINE",
    "cd src; rg DEADLINE",
    "sleep 1 & rg DEADLINE",
    "cd src&&rg DEADLINE",
    "cat notes.txt | grep fix || rg DEADLINE",
    "cat notes.txt | rg DEADLINE",
    "cat notes.txt | grep -r DEADLINE .",
    "grep -r DEADLINE . | head",
    "echo x && grep -r DEADLINE .",
    "ls|grep -r x",
    "egrep -r DEADLINE .",
    "fgrep -R DEADLINE .",
    "ripgrep DEADLINE",
    "fd DEADLINE",
    "fd -name x",
    "fdfind -path x",
    "find . -type f",
    "find . -path '*x*'",
    "find . -ipath x",
    "find . -regex x",
    "find . -iregex x",
    "grep --dereference-recursive DEADLINE .",
    "grep -Rn DEADLINE .",
    "grep -nr DEADLINE",
    "grep --regexp=DEADLINE -r .",
    "grep -xn DEADLINE notes.txt",
    "grep -- -r",
    "grep -r",
    "/usr/bin/grep -r DEADLINE .",
    "./rg DEADLINE",
    "bin/find . -name x",
    "grep/ -r DEADLINE .",
    "git grep -r DEADLINE",
    "xargs grep -r DEADLINE",
    "..",
    "rg",
    "rg -- DEADLINE",
    "rg -t py DEADLINE",
    "rg DEADLINE ''",
    "rg DEADLINE app.py",
    "rg DEADLINE app.py src",
    "rg DEADLINE src/",
    "rg DEADLINE /",
    "rg DEADLINE missing.txt",
    "rg DEADLINE 'a\x00b'",
    "rg DEADLINE\nls",
    "  rg\tDEADLINE  ",
    "rg 'DEAD LINE' src",
    'rg "DEADLINE" src',
    "rg 'DEADLINE",
    'rg "DEADLINE',
    "rg DEADLINE \\",
    'rg "a\\"b" src',
    "rg a\\ b src",
]
# JSON string text with escapes: lone surrogates in the command.
SURROGATE_COMMANDS = [
    b"rg \\ud800",
    b"rg x \\udcffdir",
    b"rg x \\ud800dir",
    b"/\\ud800/grep -r x .",
    b"gr\\ud800ep -r x .",
    b"rg x '\\udcffdir'",
]
GREP_PATHS: list[Json] = [
    "app.py/",
    "app.py/.",
    "./app.py",
    "src/../app.py",
    "src//../app.py",
    "src",
    "missing.py",
    "",
    7,
    None,
    "app.py\x00",
    "app.py/x",
]


def shapes_cases() -> list[Case]:
    """The shell and native shapes that no reference test reaches."""
    setup = [["dir", "src"], ["file", "app.py", "x = 1\n"]]
    cases = [Case(f"shell {text!r}", [checked(shell(text))], setup) for text in SHELL_SHAPES]
    escaped = [
        *setup,
        ["dir", "\xffdir"],
        ["file", "\xffapp.py", "x\n"],
        ["file", "\xffd/app.py", ""],
    ]
    cases += [
        Case(f"shell {latin(command)!r}", [checked(raw_shell(command))], escaped)
        for command in SURROGATE_COMMANDS
    ]
    for value in [5, "", None]:
        cases.append(Case(f"shell command {value!r}", [checked(shell(value))], setup))
    for path in GREP_PATHS:
        steps = [checked(native("exact", "Grep", pattern="x", path=path))]
        cases.append(Case(f"grep path {path!r}", steps, setup))
    for path, cwd in [
        (b"\\udcffapp.py", PLACEHOLDER.encode()),
        (b"\\ud800", PLACEHOLDER.encode()),
        (b"app.py", PLACEHOLDER.encode() + b"/\\udcffd"),
        (b"app.py", PLACEHOLDER.encode() + b"/\\ud800d"),
    ]:
        name = f"grep path {latin(path)!r} in {latin(cwd)!r}"
        cases.append(Case(name, [checked(raw_grep(path, cwd))], escaped))
    for given in [None, "", 5, f"{PLACEHOLDER}/src"]:
        payload: dict[str, Json] = {
            "session_id": "s",
            "tool_name": "Grep",
            "tool_input": {"pattern": "x", "path": "app.py"},
        }
        if given is not None:
            payload["cwd"] = given
        cases.append(Case(f"grep with payload cwd {given!r}", [checked(payload)], setup))
    absolute = native("exact", "Grep", pattern="x", path=f"{PLACEHOLDER}/app.py")
    absolute["cwd"] = f"{PLACEHOLDER}/src"
    cases.append(Case("grep with an absolute path", [checked(absolute)], setup))
    glob = native("exact", "Glob", pattern="*", path="app.py")
    cases.append(Case("glob with a file path", [checked(glob)], setup))
    loop = native("exact", "Grep", pattern="x", path="loop")
    cases.append(Case("a symlink loop is not a file", [checked(loop)], [["link", "loop", "loop"]]))
    return cases


RECORD_VALUES: list[tuple[str, bytes]] = [
    ("a big integer", b"123456789012345678901234567890"),
    ("a negative zero integer", b"-0"),
    ("a float", b"1.5"),
    ("a small float", b"1e-7"),
    ("a large float", b"1e16"),
    ("an overflowing float", b"1e999"),
    ("a negative infinity", b"-Infinity"),
    ("NaN", b"NaN"),
    ("a negative zero float", b"-0.0"),
    ("true", b"true"),
    ("null", b"null"),
    (
        "non-ASCII text",
        json.dumps(f"{NAIVE} {LINE_SEPARATOR} {GRINNING}", ensure_ascii=False).encode(),
    ),
    ("an escaped lone surrogate", b'"\\ud800x"'),
    ("an escaped pair", b'"\\ud83d\\ude00"'),
    ("a nested value", b'{"b": [1, {"a": null}], "a": 2, "b": 3}'),
    ("an empty object and array", b"[{}, []]"),
    ("control characters", b'"\\u0000\\u001f\\t\\n\\"\\\\/"'),
]


def records_cases() -> list[Case]:
    """One log line for each event and reason, with JSON values of each type in the fields."""
    cases = []
    for label, value in RECORD_VALUES:
        tool = b'{"session_id": "s", "tool_name": ' + value + b', "tool_input": {"pattern": 1}}'
        pattern = b'{"session_id": "s", "tool_name": "Glob", "tool_input": {"pattern": '
        session = b'{"session_id": ' + value + b', "tool_name": "Glob"}'
        operation = b'{"session_id": "s", "tool_name": "mcp__agentless__orient", "tool_input": '
        cases.append(Case(f"{label} as the tool", [checked(tool), marked(tool)]))
        cases.append(Case(f"{label} as the pattern", [checked(pattern + value + b"}}")]))
        cases.append(Case(f"{label} as the session id", [checked(session)]))
        steps = [marked(operation + b'{"operation": ' + value + b"}}")]
        cases.append(Case(f"{label} as the operation", steps))
    duplicate_mark = (
        b'{"session_id": "dup", "tool_name": "x", "tool_input": {"operation": "map"}, '
        b'"tool_name": "mcp__agentless__orient"}'
    )
    duplicate_check = (
        b'{"session_id": "dup", "tool_name": "Grep", "tool_input": {"pattern": 1, "pattern": "x"}}'
    )
    cases += [
        Case("a deny with no log", [checked(native("quiet", "Glob", pattern="*"), log=None)]),
        Case(
            "a deny with an empty log variable", [checked(native("q", "Glob", pattern="*"), log="")]
        ),
        Case(
            "a second unlock rewrites the marker",
            [
                marked(structural("again", "orient", operation="map")),
                marked(structural("again", "read", operation="slice")),
            ],
        ),
        Case(
            "a non-ASCII session id",
            [marked(structural("sess-" + NAIVE, "orient", operation="path"))],
        ),
        Case(
            "a duplicate key keeps its last value",
            [marked(duplicate_mark), checked(duplicate_check)],
        ),
    ]
    return cases


def nested(depth: int) -> bytes:
    return b"[" * depth + b"]" * depth


def pattern_payload(value: bytes) -> bytes:
    return b'{"session_id": "s", "tool_name": "Glob", "tool_input": {"pattern": ' + value + b"}}"


STDIN_TEXTS: list[tuple[str, bytes]] = [
    ("stdin that is not UTF-8", b'{"session_id": "\xff"}'),
    ("stdin with an encoded surrogate", b'{"session_id": "\xed\xa0\x80"}'),
    ("stdin with a byte order mark", b"\xef\xbb\xbf{}"),
    ("empty stdin", b""),
    ("blank stdin", b"  "),
    ("a newline", b"\n"),
    ("null", b"null"),
    ("a number", b"5"),
    ("a string", b'"x"'),
    ("an array", b"[]"),
    ("trailing data", b"{} {}"),
    ("a trailing comma", b'{"a": 1,}'),
    ("an unterminated string", b'{"a": "x'),
    ("a bad escape", b'{"a": "\\x"}'),
    ("a control character", b'{"a": "\x01"}'),
    ("a carriage return between tokens", b'{"session_id":\r"s"}'),
    ("an array at the deepest nesting", nested(MAX_DEPTH)),
    ("an array one level deeper", nested(MAX_DEPTH + 1)),
    ("a pattern at the deepest nesting", pattern_payload(nested(MAX_DEPTH - 2))),
    ("a pattern one level deeper", pattern_payload(nested(MAX_DEPTH - 1))),
    ("an integer of the most digits", pattern_payload(b"1" * INT_DIGITS)),
    ("an integer of one digit more", pattern_payload(b"1" * (INT_DIGITS + 1))),
]
SURROGATE_SESSION_TAILS: list[tuple[str, str]] = [
    ("Grep", '"tool_name": "Grep", "tool_input": {"pattern": "x"}}'),
    ("Bash", '"tool_name": "Bash", "tool_input": {"command": "rg x"}}'),
    ("Bash with no command", '"tool_name": "Bash", "tool_input": {}}'),
    ("Read", '"tool_name": "Read"}'),
    ("orient map", '"tool_name": "mcp__agentless__orient", "tool_input": {"operation": "map"}}'),
    ("capabilities", '"tool_name": "mcp__agentless__capabilities"}'),
]


def failures_cases() -> list[Case]:
    """The branches whose error the reference suppresses, and the payload text edge cases."""
    glob = native("f", "Glob", pattern="*")
    blocked = "blocker/gate.jsonl"
    blocker = [["file", "blocker", ""]]
    health = structural("f", "orient", operation="health")
    cases = [
        Case("the log parent is a file on a deny", [checked(glob, log=blocked)], blocker),
        Case(
            "the log parent is a file on an allow",
            [checked(native("f", "Read", pattern="*"), log=blocked)],
            blocker,
        ),
        Case("the log is a directory", [checked(glob, log="logdir")], [["dir", "logdir"]]),
        Case(
            "the log parent is a file on a mark",
            [
                marked(structural("f", "orient", operation="map"), log=blocked),
                marked(health, log=blocked),
            ],
            blocker,
        ),
        Case("a deny whose stderr write fails", [checked(glob, stderr_fails=True)]),
    ]
    cases += [Case(label, [checked(text), marked(text)]) for label, text in STDIN_TEXTS]
    for session in ["\\ud800", "\\udcff"]:
        for label, tail in SURROGATE_SESSION_TAILS:
            text = ('{"session_id": "a' + session + '", ' + tail).encode("ascii")
            hook = marked if "agentless" in tail else checked
            cases.append(Case(f"a session id with {session} and {label}", [hook(text)]))
    for label, tool in [("an array", ["Grep"]), ("an object", {"a": 1})]:
        value = {"session_id": "s", "tool_name": tool, "tool_input": {"pattern": "x"}}
        steps = [checked(value), marked(value), checked({"tool_name": tool})]
        cases.append(Case(f"{label} as the tool name", steps))
    cases += removed_cwd_cases()
    cases += mode_cases()
    cases += marker_cases()
    return cases


def removed_cwd_cases() -> list[Case]:
    def grep(**tool_input: Json) -> dict[str, Json]:
        return {
            "session_id": "s",
            "tool_name": "Grep",
            "tool_input": {"pattern": "x", **tool_input},
        }

    no_cwd_shell = {"session_id": "s", "tool_name": "Bash", "tool_input": {"command": "ls"}}
    return [
        Case("a removed cwd and a relative Grep path", [checked(grep(path="app.py"), cwd=None)]),
        Case("a removed cwd and an absolute Grep path", [checked(grep(path="/"), cwd=None)]),
        Case("a removed cwd and no Grep path", [checked(grep(), cwd=None)]),
        Case("a removed cwd and a shell command", [checked(no_cwd_shell, cwd=None)]),
        Case(
            "a removed cwd and a shell command with a payload cwd",
            [checked(shell("rg DEADLINE", "s"), cwd=None)],
        ),
        Case(
            "a removed cwd and a structural call",
            [marked(structural("s", "orient", operation="map"), cwd=None)],
        ),
    ]


def mode_cases() -> list[Case]:
    locked: list[Op] = [["dir", "locked/sub"], ["mode", "locked", 0o000]]
    steps = [
        checked(native("m", "Glob", pattern="*")),
        checked(shell("rg x", "m")),
        marked(structural("m", "orient", operation="map")),
    ]
    return [
        Case(
            "an unsearchable Grep path",
            [checked(native("m", "Grep", pattern="x", path="locked/app.py"))],
            locked,
        ),
        Case("an unsearchable rg operand", [checked(shell("rg x locked/sub", "m"))], locked),
        Case(
            "a Grep path name too long",
            [checked(native("m", "Grep", pattern="x", path=LONG_NAME))],
        ),
        Case("an rg operand name too long", [checked(shell(f"rg x {LONG_NAME}", "m"))]),
        Case(
            "an unsearchable marker directory",
            steps,
            [["dir", MARKERS], ["mode", MARKERS, 0o000]],
        ),
        Case("a read-only marker directory", steps, [["dir", MARKERS], ["mode", MARKERS, 0o500]]),
    ]


def marker_cases() -> list[Case]:
    name = marker_name("m")
    glob = checked(native("m", "Glob", pattern="*"))
    orient = marked(structural("m", "orient", operation="map"))
    return [
        Case("the marker directory is a file", [glob, orient], [["file", MARKERS, ""]]),
        Case("the marker path is a directory", [orient, glob], [["dir", f"{MARKERS}/{name}"]]),
        Case(
            "the marker is a dangling symlink",
            [glob],
            [["dir", MARKERS], ["link", f"{MARKERS}/{name}", "missing"]],
        ),
        Case(
            "the probe of this process is a directory",
            [glob],
            [["dir", f"{MARKERS}/.write-probe-{PID_MASK}"]],
        ),
        Case(
            "the probe of another process is a directory",
            [glob],
            [["dir", f"{MARKERS}/.write-probe-1"]],
        ),
    ]


MARK_TOOLS: list[tuple[str, str, Json]] = [
    ("a numeric operation", "mcp__agentless__orient", {"operation": 5}),
    ("no operation", "mcp__agentless__orient", {}),
    ("a tool input that is a list", "mcp__agentless__symbols", ["find"]),
    ("another server", "mcp__other__orient", {"operation": "map"}),
    ("a bare tool name", "orient", {"operation": "map"}),
    ("an operation in capitals", "mcp__agentless__orient", {"operation": "MAP"}),
    (
        "references with an operation",
        "mcp__agentless__find_referencing_symbols",
        {"operation": "x"},
    ),
    ("a v1 name with an operation", "mcp__agentless__repo_map", {"operation": "x"}),
    ("a doubled prefix", "mcp__agentless__mcp__agentless__orient", {"operation": "map"}),
]


def mark_cases() -> list[Case]:
    """The gate-mark payloads that no reference test reaches."""
    cases = [
        Case(f"a mark of {latin(text)!r}", [marked(text)])
        for text in [b"{", b"[]", b'"x"', b"5", b"null", b"{}"]
    ]
    for session in [None, "", 5]:
        value: dict[str, Json] = {
            "tool_name": "mcp__agentless__orient",
            "tool_input": {"operation": "map"},
        }
        if session is not None:
            value["session_id"] = session
        cases.append(Case(f"a mark with session id {session!r}", [marked(value)]))
    for label, tool, tool_input in MARK_TOOLS:
        value = {"session_id": "k", "tool_name": tool, "tool_input": tool_input}
        cases.append(Case(f"a mark with {label}", [marked(value), checked(broad("k"))]))
    return cases


def cross_cases() -> list[Case]:
    """A marker that the reference mark hook wrote unlocks the gate."""
    op = reference_marker("crossed", "orient", "map")
    return [
        Case("a reference marker unlocks Grep", [checked(broad("crossed"))], [op]),
        Case("a reference marker unlocks Bash", [checked(shell("rg DEADLINE", "crossed"))], [op]),
    ]


def build(cases: Callable[[], list[Case]]) -> Callable[[], Fixture]:
    return lambda: {"cases": [outcome(case) for case in cases()]}


def main() -> int:
    if os.geteuid() == 0:
        print("refused: run as a user; root ignores the directory modes", file=sys.stderr)
        return 2
    return write_or_check(
        "Write the search gate fixtures from the reference gate hooks.",
        FIXTURES,
        {
            "tests": build(tests_cases),
            "shapes": build(shapes_cases),
            "records": build(records_cases),
            "failures": build(failures_cases),
            "mark": build(mark_cases),
            "cross": build(cross_cases),
            "clock": lambda: {"wall_ns": [WALL_NS]},
        },
    )


if __name__ == "__main__":
    raise SystemExit(main())
