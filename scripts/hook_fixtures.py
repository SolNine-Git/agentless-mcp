"""Write the hook crate's test fixtures: each input with what the Python erosion gate does with it.

Development-time only. Run it from the repository root with the project environment, and with
the grammar libraries copied to a scratch directory, so that no run reads or writes the user's
grammar cache:

    TREE_SITTER_LANGUAGE_PACK_CACHE_DIR=<scratch copy> uv run python scripts/hook_fixtures.py

The scenarios run the Python hook as a process, with this branch's agentless-mcp as its lint.
Every run gets its own HOME, cache home and log under a temporary directory. With --check, the
script compares its output with the committed fixtures and writes nothing.
"""

import hashlib
import importlib.util
import io
import json
import os
import random
import subprocess
import sys
import tempfile
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from pathlib import Path
from types import ModuleType
from typing import Any

from fixture_files import (
    Fixture,
    Identity,
    hermetic_env,
    refuse_real_caches,
    run_git,
    write_or_check,
)

from agentless_mcp.core import projectconfig

REPO = Path(__file__).resolve().parents[1]
FIXTURES = REPO / "crates" / "hook" / "fixtures"
HOOK_PATH = REPO / "contrib" / "hooks" / "agentless_erosion_gate.py"
LINT_BINARY = REPO / ".venv" / "bin" / "agentless-mcp"
GRAMMAR_OVERRIDE = "TREE_SITTER_LANGUAGE_PACK_CACHE_DIR"
WALL_SECONDS = 1_760_000_000.0
IDENTITY = Identity("hook tests", "hook@example.invalid", "2026-02-01T00:00:00Z")
SEED = 20261011
# The seeded Edit sample: the share of Edits on a missing file, and the share with replace_all.
MISSING_SHARE = 0.05
REPLACE_ALL_SHARE = 0.5


def _load_hook() -> ModuleType:
    spec = importlib.util.spec_from_file_location("agentless_erosion_gate", HOOK_PATH)
    if spec is None or spec.loader is None:
        message = f"refused: {HOOK_PATH} does not load as a module"
        raise SystemExit(message)
    module = importlib.util.module_from_spec(spec)
    sys.modules["agentless_erosion_gate"] = module
    spec.loader.exec_module(module)
    return module


hook = _load_hook()


def latin(data: bytes) -> str:
    """Return ``data`` as a string with one character per byte."""
    return data.decode("latin-1")


def utf8_or_none(text: str) -> str | None:
    """Return ``text``, or None when it holds a lone surrogate."""
    try:
        text.encode("utf-8")
    except UnicodeEncodeError:
        return None
    return text


# The text forms.

CODEC_INPUTS = [
    b"\xff{}",
    b'{"x": "\xe2\x82"}',
    b"\xf0\x90\x28",
    b"ab\xe2\x82",
    b"ab\xe2",
    b"\xc0\xaf",
    b"\xed\xa0\x80",
    b"\xf4\x90\x80\x80",
    b"a\x80",
    b"\xe0\x80\x80",
    b"\xf0\x9f\x98",
]
ERRNOS = [2, 5, 13, 20, 21, 40]
LAST_TEXTS = [
    "",
    "\n\n",
    "a\nb\n",
    "  x  \n\n",
    "fatal: not a git repository\n",
    "x" * 400,
    "a\r\nb",
    "\u00e9" * 350,
    "one\n   \ntwo  \n",
]
NANOS = [
    1_760_000_000 * 10**9,
    1_760_000_000_123_456_789,
    1,
    999_999_999,
    10**9,
    1_712_345_678_900_000_000,
]


def _codec_text(data: bytes) -> str:
    try:
        data.decode("utf-8")
    except UnicodeDecodeError as error:
        return str(error)
    message = f"refused: {data!r} is UTF-8"
    raise SystemExit(message)


def _seconds(ns: int) -> float:
    # CPython's _PyTime_AsSecondsDouble, which time.time() returns.
    if ns % 10**9 == 0:
        return float(ns // 10**9)
    return float(ns) / 1e9


def _os_error_line(errno: int) -> str:
    error = OSError(errno, os.strerror(errno))
    return f"{type(error).__name__}: {error}"


def text_fixture() -> Fixture:
    """Return the decode, OS error, last-line, grouping, time and digest texts."""
    paths = ["/r/a.py", "/r/\u00fc.py", "/r/it's.py"]
    return {
        "codec": [[latin(data), _codec_text(data)] for data in CODEC_INPUTS],
        "os_error": [
            [errno, path, str(OSError(errno, os.strerror(errno), path))]
            for errno in ERRNOS
            for path in paths
        ],
        "os_error_line": [[errno, _os_error_line(errno)] for errno in [*ERRNOS, 11, 32, 110]],
        "last_line": [[text, hook._last_line(text)] for text in LAST_TEXTS],
        "grouped": [[n, f"{n:,}"] for n in [0, 7, 999, 1000, 1_000_000, 1_234_567]],
        "seconds": [[str(ns), repr(_seconds(ns))] for ns in NANOS],
        "sha256": [
            [text, hashlib.sha256(text.encode("utf-8")).hexdigest()]
            for text in ["", "x = 1\n", "\u00e9\r\n"]
        ],
    }


# The payload.


def _document(**fields: object) -> dict[str, object]:
    base: dict[str, object] = {
        "session_id": "s1",
        "transcript_path": "/transcripts/t.jsonl",
        "tool_use_id": "u1",
    }
    base.update(fields)
    return base


def _write(path: str, content: object = "x = 1\n", **extra: object) -> dict[str, object]:
    return _document(tool_name="Write", tool_input={"file_path": path, "content": content}, **extra)


def _edit(path: str, old: object, new: object, **tool: object) -> dict[str, object]:
    tool_input = {"file_path": path, "old_string": old, "new_string": new, **tool}
    return _document(tool_name="Edit", tool_input=tool_input)


def _without(document: dict[str, object], key: str) -> dict[str, object]:
    return {name: value for name, value in document.items() if name != key}


def _with_input(document: dict[str, object], **tool: object) -> dict[str, object]:
    tool_input = document["tool_input"]
    if not isinstance(tool_input, dict):
        message = "refused: the document has no tool_input object"
        raise SystemExit(message)
    return {**document, "tool_input": {**tool_input, **tool}}


PAYLOAD_DOCUMENTS: list[tuple[str, dict[str, object]]] = [
    ("write", _write("/r/a.py")),
    ("edit", _edit("/r/a.py", "a", "b")),
    ("edit_replace_all", _edit("/r/a.py", "a", "b", replace_all=True)),
    ("edit_replace_all_false", _edit("/r/a.py", "a", "b", replace_all=False)),
    ("relative_with_cwd", _write("a.py", cwd="/r/sub")),
    ("relative_dot_with_cwd", _write("./x/../a.py", cwd="/r/")),
    ("empty_path_with_cwd", _write("", cwd="/r")),
    ("relative_without_cwd", _write("a.py")),
    ("relative_cwd_not_str", _write("a.py", cwd=3)),
    ("double_slash", _write("//r//a.py")),
    ("unicode_path", _write("/r/\u00e9t\u00e9.py")),
    ("nul_path", _write("/r/a\x00b.py")),
    ("extra_fields", _write("/r/a.py", hook_event_name="PreToolUse", permission_mode="x")),
    ("multi_edit", _document(tool_name="MultiEdit", tool_input={})),
    ("read_tool", _document(tool_name="Read", tool_input={"file_path": "/r/a.py"})),
    ("tool_name_int", _document(tool_name=1)),
    ("tool_name_missing", _without(_write("/r/a.py"), "tool_name")),
    ("tool_input_list", _document(tool_name="Write", tool_input=["a"])),
    ("tool_input_missing", _document(tool_name="Edit")),
    ("file_path_int", _with_input(_write("/r/a.py"), file_path=5)),
    ("file_path_missing", _document(tool_name="Write", tool_input={"content": ""})),
    ("session_id_missing", _without(_write("/r/a.py"), "session_id")),
    ("session_id_int", {**_write("/r/a.py"), "session_id": 5}),
    ("transcript_path_null", {**_write("/r/a.py"), "transcript_path": None}),
    ("tool_use_id_missing", _without(_write("/r/a.py"), "tool_use_id")),
    ("content_missing", _document(tool_name="Write", tool_input={"file_path": "/r/a.py"})),
    ("content_int", _write("/r/a.py", content=3)),
    ("content_surrogate", _write("/r/a.py", content="\ud800")),
    ("replace_all_str", _edit("/r/a.py", "a", "b", replace_all="yes")),
    ("replace_all_int", _edit("/r/a.py", "a", "b", replace_all=1)),
    ("old_string_null", _with_input(_edit("/r/a.py", "a", "b"), old_string=None)),
    ("new_string_null", _edit("/r/a.py", "a", None)),
    ("edit_no_strings", _document(tool_name="Edit", tool_input={"file_path": "/r/a.py"})),
    ("big_int_extra", _write("/r/a.py", n=10**40)),
    ("nan_extra", _write("/r/a.py", n=float("nan"))),
]
PAYLOAD_RAW: list[tuple[str, bytes]] = [
    (
        "duplicate_keys",
        (
            b'{"tool_name": "Read", "tool_name": "Write", "tool_input": {"file_path": "/r/a.py", '
            b'"content": ""}, "session_id": "s", "transcript_path": "t", "tool_use_id": "u"}'
        ),
    ),
    ("not_json", b"{nope"),
    ("empty", b""),
    ("array", b"[1, 2]"),
    ("string", b'"x"'),
    ("trailing_comma", b'{"tool_name": "Write",}'),
    ("bom", b'\xef\xbb\xbf{"tool_name": "Write"}'),
    ("not_utf8", b'{"tool_name": "\xff"}'),
    ("cut_utf8", b'{"tool_name": "\xe2\x82'),
    ("depth_9998", b"[" * 9998 + b"]" * 9998),
    ("depth_9999", b"[" * 9999 + b"]" * 9999),
    ("object_depth_9999", b'{"a": ' * 9999 + b"1" + b"}" * 9999),
    # The integer digit limit: 4300 digits read, one more refuses the document, sign excluded.
    *[
        (name, json.dumps(_write("/r/a.py")).encode("ascii")[:-1] + b', "n": ' + number + b"}")
        for name, number in [
            ("int_4300_digits", b"1" * 4300),
            ("int_4301_digits", b"1" * 4301),
            ("negative_int_4301_digits", b"-" + b"1" * 4301),
        ]
    ],
]
# Each repeated payload: one character and its count.
PAYLOAD_REPEATED: list[tuple[str, int]] = [
    ("cap_exactly", hook.MAX_PAYLOAD_BYTES),
    ("cap_over", hook.MAX_PAYLOAD_BYTES + 1),
]


def _change(payload: Any) -> list[object]:
    change = payload.change
    if isinstance(change, str):
        return ["write", utf8_or_none(change)]
    old, new = utf8_or_none(change.old_string), utf8_or_none(change.new_string)
    return ["edit", old, new, change.replace_all]


def _payload_result(raw: bytes) -> tuple[list[object], str | None, dict[str, object]]:
    record = hook._blank_record()
    path = None
    try:
        payload = hook._read_payload(io.BytesIO(raw), record)
        path = str(payload.file_path)
        result: dict[str, object] = {"change": _change(payload)}
    except hook._HaltError as halt:
        result = {"halt": [halt.outcome, halt.reason]}
    except RecursionError as error:  # the hook's internal-error branch; JSON depth raises it
        result = {"halt": [hook.DEGRADED, f"internal error: {type(error).__name__}: {error}"]}
    identity = [record[key] for key in hook.IDENTITY_KEYS]
    return identity, path, result


def payload_case(name: str, shown: object, raw: bytes) -> list[object]:
    """Return one payload case: what the Python hook reads from these bytes."""
    identity, path, result = _payload_result(raw)
    return [name, shown, {"identity": identity, "path": path, "result": result}]


def payload_fixture() -> Fixture:
    """Return the payload cases."""
    cases = [
        payload_case(name, latin(raw), raw)
        for name, raw in [
            *[(name, json.dumps(doc).encode("ascii")) for name, doc in PAYLOAD_DOCUMENTS],
            *PAYLOAD_RAW,
        ]
    ]
    cases += [payload_case(name, [" ", count], b" " * count) for name, count in PAYLOAD_REPEATED]
    return {"payload": cases}


# The proposed text.


def edit_case(
    kind: str, old: str | None, first: str, second: str | None, replace_all: bool
) -> list[object]:
    """Return one proposal case: the text Claude Code writes, or why it fails the Edit."""
    change: object = first
    if kind != "write":
        change = hook.EditInput(first, second, replace_all)
    tool = "Write" if kind == "write" else "Edit"
    payload = hook.Payload("s", "t", "u", tool, Path("/r/a.py"), change)
    try:
        proposal = hook._proposal(payload, old)
        expected: list[object] = ["text", proposal.text, proposal.eol]
    except hook._HaltError as halt:
        if halt.outcome != hook.SKIPPED:
            message = f"refused: {halt.reason} is not a skip"
            raise SystemExit(message) from halt
        expected = ["skip", halt.reason]
    return [kind, old, first, second, replace_all, expected]


EDIT_PICKED: list[tuple[str, str | None, str, str | None, bool]] = [
    ("write", None, "x = 1\n", None, False),
    ("write", "a\r\nb\r\n", "a\nb\n", None, False),
    ("edit", None, "a", "b", False),
    ("edit", "a\r\nb\r\nc\n", "b", "B", False),
    ("edit", "a\r\nb\n", "b", "B", False),
    ("edit", "a\nb\n", "b", "B", False),
    ("edit", "x\nx\n", "x", "y", False),
    ("edit", "x\nx\n", "x", "y", True),
    ("edit", "x\n", "", "y", False),
    ("edit", "x\n", "x", "x", False),
    ("edit", "x\n", "z", "y", False),
    ("edit", "a\r\nb\r\n", "a\r\nb", "c", False),
    ("edit", "a\r\nb\r\n", "a\nb", "c\nd", False),
    ("edit", "aaaa", "aa", "b", True),
    ("edit", "\u00e9\u00e9\n", "\u00e9", "e", True),
]


def edit_fixture() -> Fixture:
    """Return hand-picked proposal cases, then a seeded sample over a small alphabet."""
    cases = [edit_case(*case) for case in EDIT_PICKED]
    rng = random.Random(SEED)
    alphabet = ["a", "b", "\r", "\n"]
    old_strings = ["", "a", "b", "ab", "\n", "a\n", "\r\n", "b\r", "\r", "x"]
    new_strings = ["", "a", "x", "x\n", "\r\n", "ab"]
    for _ in range(3000):
        missing = rng.random() < MISSING_SHARE
        old = "".join(rng.choices(alphabet, k=rng.randint(0, 7)))
        first, second = rng.choice(old_strings), rng.choice(new_strings)
        replace_all = rng.random() < REPLACE_ALL_SHARE
        cases.append(edit_case("edit", None if missing else old, first, second, replace_all))
    return {"edit": cases}


# The modes.

GATE_DOCUMENTS: list[tuple[str, object]] = [
    (
        "valid_all",
        {
            "erosion_gate": {
                "complexity_delta": "block",
                "contract_drift": "off",
                "clone_delta": "advisory",
            }
        },
    ),
    ("valid_one", {"erosion_gate": {"clone_delta": "block"}}),
    ("empty_object", {}),
    ("gate_empty", {"erosion_gate": {}}),
    ("gate_list", {"erosion_gate": ["block"]}),
    ("gate_string", {"erosion_gate": "block"}),
    ("gate_null", {"erosion_gate": None}),
    ("bad_mode_string", {"erosion_gate": {"complexity_delta": "blocking"}}),
    ("bad_mode_int", {"erosion_gate": {"complexity_delta": 1}}),
    ("bad_mode_bool", {"erosion_gate": {"contract_drift": True}}),
    ("bad_mode_null", {"erosion_gate": {"clone_delta": None}}),
    ("bad_mode_list", {"erosion_gate": {"clone_delta": ["block"]}}),
    ("bad_mode_upper", {"erosion_gate": {"clone_delta": "BLOCK"}}),
    ("unknown_check", {"erosion_gate": {"complexity_delta": "block", "dead_code": "block"}}),
    ("unknown_only", {"erosion_gate": {"x": "off", "y": "on"}}),
    ("two_bad", {"erosion_gate": {"complexity_delta": "x", "clone_delta": "y"}}),
    ("other_bad_key", {"map_budget": 5, "erosion_gate": {"complexity_delta": "block"}}),
    ("unknown_top_key", {"nope": 1, "erosion_gate": {"complexity_delta": "off"}}),
    ("bad_globs", {"boundary_globs": "x", "erosion_gate": {"clone_delta": "off"}}),
]
GATE_PAD = b'{"erosion_gate": {"clone_delta": "block"}, "pad": "'
GATE_RAW: list[tuple[str, bytes]] = [
    ("array", b"[1, 2]"),
    ("string", b'"x"'),
    ("not_json", b"{nope"),
    ("trailing_comma", b'{"erosion_gate": {},}'),
    ("bom", b'\xef\xbb\xbf{"erosion_gate": {"clone_delta": "block"}}'),
    ("not_utf8", b'{"erosion_gate": {"clone_delta": "bl\xffck"}}'),
    ("empty_file", b""),
    (
        "duplicate_keys",
        b'{"erosion_gate": {"clone_delta": "block"}, "erosion_gate": {"clone_delta": "off"}}',
    ),
    ("at_cap", GATE_PAD + b"x" * (65536 - len(GATE_PAD) - 2) + b'"}'),
    ("over_cap", GATE_PAD + b"x" * (65537 - len(GATE_PAD) - 2) + b'"}'),
]
ENV_VALUES = [None, "", "advisory", "block", "BLOCK", "nope", "bl\u00f6ck"]


def rust_config_warnings(root: Path, env_value: str | None) -> list[str]:
    """Return the config_warnings the Rust hook logs: lint's file and erosion_gate texts (D2)."""
    loaded = projectconfig.load(root)
    if loaded.present:
        kept = [warning for warning in loaded.warnings if warning.startswith("erosion_gate")]
    else:
        kept = list(loaded.warnings)
    _, env_warnings = hook.resolve_modes({}, env_value)
    return [*kept, *env_warnings]


def modes_case(name: str, config: bytes | None, env_value: str | None) -> list[object]:
    """Return one modes case: the Python hook's modes, and the warnings the Rust hook logs."""
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch).resolve() / "repo"
        root.mkdir()
        if config is not None:
            (root / projectconfig.CONFIG_FILENAME).write_bytes(config)
        document, _ = hook._config_document(root)
        entries, _ = hook.repo_modes(document)
        modes, _ = hook.resolve_modes(entries, env_value)
        warnings = rust_config_warnings(root, env_value)
    shown = None if config is None else latin(config)
    return [name, shown, env_value, json.dumps(modes), warnings]


def modes_fixture() -> Fixture:
    """Return the modes cases: each config with no env value, then each env value."""
    configs: list[tuple[str, bytes | None]] = [("absent", None)]
    configs += [(name, json.dumps(document).encode()) for name, document in GATE_DOCUMENTS]
    configs += GATE_RAW
    cases = [modes_case(name, config, None) for name, config in configs]
    entry = json.dumps(GATE_DOCUMENTS[1][1]).encode()
    for env_value in ENV_VALUES:
        cases.append(modes_case(f"env {env_value!r}", None, env_value))
        cases.append(modes_case(f"env {env_value!r} with an entry", entry, env_value))
    return {"modes": cases}


# Where the call writes.

TIER1_NAMES = [
    *["a.py", "a.PY", ".py", "a.", "a..py", "a.tsx", "a.md", "Makefile", "a.py.bak"],
    *["a.c", "a.h", "a.hpp", "a.sh", "a.bash", "a.lua", "a.rb", "a.java", "a.go", "a.rs"],
    *["a.ts", "a.js", "a.mjs", "a.cjs", "a.cc", "a.cxx", "a.cpp", "a.hxx", "a.kt", "a.json"],
    *["a.toml", "a.yaml", "a.sql", "a.swift", "a.scala", "a.cs", "a.php", "a.hcl", "a.tf"],
    *["x.d.ts", "\u00e9.py"],
]
RELATIVE = [
    ("/r/a.py", "/r"),
    ("/r/s/a.py", "/r"),
    ("/rr/a.py", "/r"),
    ("/r", "/r"),
    ("/x/a.py", "/r"),
    ("/r/a/../b.py", "/r"),
    ("/", "/"),
    ("/a.py", "/"),
]


def _relative(resolved: str, root: str) -> str | None:
    try:
        return str(hook._relative(Path(resolved), Path(root)))
    except hook._HaltError as halt:
        if halt.reason != "the resolved target is outside the repository":
            message = f"refused: {halt.reason}"
            raise SystemExit(message) from halt
        return None


def target_fixture() -> Fixture:
    """Return the tier-1 rule, the suffix list and the path below the root."""
    tier1 = [[name, Path("/repo", name).suffix in hook.TIER1_SUFFIXES] for name in TIER1_NAMES]
    return {
        "tier1": tier1,
        "suffixes": [[suffix] for suffix in sorted(hook.TIER1_SUFFIXES)],
        "relative": [[resolved, root, _relative(resolved, root)] for resolved, root in RELATIVE],
    }


# The classification and the reply.


def _row(
    check: str, severity: str, line: int = 1, detail: object = None, **fields: str
) -> dict[str, object]:
    row: dict[str, object] = {
        "check": check,
        "severity": severity,
        "message": fields.get("message", f"{check} at line {line}"),
        "path": fields.get("path", "a.py"),
        "line": line,
        "location": f"a.py:{line}",
        "evidence": fields.get("evidence", "e"),
    }
    if detail is not None:
        row["detail"] = detail
    return row


def _modes(
    complexity: str = "advisory", contract: str = "advisory", clone: str = "advisory"
) -> dict[str, str]:
    return {"complexity_delta": complexity, "contract_drift": contract, "clone_delta": clone}


UNIT = {
    "unit": "f",
    "match": "new",
    "cc_old": None,
    "cc_new": 12,
    "sloc_old": None,
    "sloc_new": 20,
    "threshold": 10,
}
NO_GLOBS = {"reason": "no_boundary_globs"}
NO_RULES = {"reason": "no_rules_for_language", "language": "python"}
NO_TAGS = {"reason": "no_tag_cache"}
CLASSIFY: list[tuple[str, dict[str, str], list[dict[str, object]]]] = [
    ("empty", _modes(), []),
    ("one_advisory", _modes(), [_row("complexity_delta", "warning", detail=UNIT)]),
    ("one_blocking", _modes("block"), [_row("complexity_delta", "warning", detail=UNIT)]),
    ("severity_advisory", _modes(), [_row("clone_delta", "advisory")]),
    (
        "block_and_advisory",
        _modes("block"),
        [_row("clone_delta", "advisory", 3), _row("complexity_delta", "warning", 1)],
    ),
    ("coverage_gap", _modes(), [_row("coverage", "not_checked", 0, message="not checked: x")]),
    ("no_boundary_globs", _modes(), [_row("contract_drift", "not_checked", 0, NO_GLOBS)]),
    ("no_rules", _modes(), [_row("contract_drift", "not_checked", 0, NO_RULES)]),
    ("no_tag_cache", _modes(), [_row("clone_delta", "not_checked", 0, NO_TAGS)]),
    (
        "off_reason_of_another_check",
        _modes(),
        [
            _row("contract_drift", "not_checked", 0, NO_TAGS),
            _row("clone_delta", "not_checked", 0, NO_GLOBS),
        ],
    ),
    (
        "finding_beside_gap",
        _modes(),
        [
            _row("coverage", "not_checked", 0, message="gap one"),
            _row("complexity_delta", "warning", 4),
        ],
    ),
    (
        "twenty_blocking",
        _modes("block"),
        [_row("complexity_delta", "warning", n) for n in range(1, 21)],
    ),
    (
        "ten_and_ten",
        _modes("block"),
        [
            *[_row("complexity_delta", "warning", n) for n in range(1, 11)],
            *[_row("clone_delta", "advisory", n) for n in range(11, 21)],
        ],
    ),
    (
        "fifteen_and_three",
        _modes("block"),
        [
            *[_row("complexity_delta", "warning", n) for n in range(1, 16)],
            *[_row("clone_delta", "advisory", n) for n in range(16, 19)],
        ],
    ),
    ("sixteen_advisory", _modes(), [_row("clone_delta", "advisory", n) for n in range(1, 17)]),
    (
        "non_ascii",
        _modes(),
        [
            _row(
                "complexity_delta",
                "warning",
                2,
                message="\u00e9l\u00e8ve \u2192 \U0001f600",
                path="d/\u00e9.py",
            )
        ],
    ),
    (
        "off_mode_listed",
        _modes(contract="off", clone="block"),
        [_row("clone_delta", "advisory", 9)],
    ),
]


def classify_case(name: str, modes: dict[str, str], rows: list[dict[str, object]]) -> list[object]:
    """Return one classification case: the reply and the record fields the decision fills."""
    checks = [check for check in hook.KNOWN_CHECKS if modes[check] != hook.OFF]
    document = {"candidates": [{"id": "a.py", "findings": rows}]}
    output = hook.parse_lint_output(json.dumps(document).encode(), checks)
    verdict = hook.classify(output, modes)
    record = hook._blank_record()
    decision = hook._decision(verdict, modes, output, record)
    expected = {
        "code": decision.code,
        "stdout": decision.stdout + "\n" if decision.stdout else "",
        "stderr": decision.stderr + "\n" if decision.stderr else "",
        "outcome": record["outcome"],
        "reason": record["reason"],
        "gaps": json.dumps(record["gaps"]),
        "off_by_config": record["off_by_config"],
        "findings": json.dumps(record["findings"]),
    }
    return [name, modes, rows, expected]


def classify_fixture() -> Fixture:
    """Return the classification cases."""
    return {"classify": [classify_case(*case) for case in CLASSIFY]}


# The scenarios: the Python hook as a process on a scratch repository.

C_BODY = ["  if (x > 0) { return 1; }", "  if (x > 2) { return 2; }", "  return 0;", "}"]
BRANCHY_LINES = {
    "py": [
        *["def f(x):", "    if x > 0:", "        return 1", "    if x > 2:", "        return 2"],
        "    return 0",
    ],
    "js": ["function f(x) {", *C_BODY],
    "ts": ["function f(x: number): number {", *C_BODY],
    "tsx": ["function f(x: number): number {", *C_BODY],
    "go": [
        *["package main", "", "func f(x int) int {", "\tif x > 0 {", "\t\treturn 1", "\t}"],
        *["\tif x > 2 {", "\t\treturn 2", "\t}", "\treturn 0", "}"],
    ],
    "rs": [
        *["fn f(x: i32) -> i32 {", "    if x > 0 {", "        return 1;", "    }"],
        *["    if x > 2 {", "        return 2;", "    }", "    0", "}"],
    ],
    "java": ["class A {", "  int f(int x) {", *[f"  {line}" for line in C_BODY], "}"],
    "c": ["int f(int x) {", *C_BODY],
    "cpp": ["int f(int x) {", *C_BODY],
    "rb": [
        *["def f(x)", "  if x > 0", "    return 1", "  end", "  if x > 2", "    return 2"],
        *["  end", "  0", "end"],
    ],
    "lua": [
        *["function f(x)", "  if x > 0 then", "    return 1", "  end", "  if x > 2 then"],
        *["    return 2", "  end", "  return 0", "end"],
    ],
    "sh": [
        *["f() {", '  if [ "$1" -gt 0 ]; then', "    echo 1", "  fi"],
        *['  if [ "$1" -gt 2 ]; then', "    echo 2", "  fi", "}"],
    ],
}
# Two branches: complexity 3, over a threshold of 1.
BRANCHY = {suffix: "\n".join(lines) + "\n" for suffix, lines in BRANCHY_LINES.items()}
TRIVIAL = {
    "py": "X = 1\n",
    "js": "const x = 1;\n",
    "ts": "const x: number = 1;\n",
    "tsx": "const x: number = 1;\n",
    "go": "package main\n\nvar x = 1\n",
    "rs": "const X: i32 = 1;\n",
    "java": "class A {\n  int x = 1;\n}\n",
    "c": "int x = 1;\n",
    "cpp": "int x = 1;\n",
    "rb": "X = 1\n",
    "lua": "local x = 1\n",
    "sh": "x=1\n",
}
FILE_NAMES = {"java": "A.java"}
CONFIG = ".agentless-mcp.json"
THRESHOLD_ONE = json.dumps({"complexity_threshold": 1})
BLOCK_AT_ONE = json.dumps(
    {"complexity_threshold": 1, "erosion_gate": {"complexity_delta": "block"}}
)
ALL_OFF = json.dumps(
    {"erosion_gate": {"complexity_delta": "off", "contract_drift": "off", "clone_delta": "off"}}
)
CLONE_OFF = json.dumps({"erosion_gate": {"clone_delta": "off"}})

# name, setup steps, payload, gate variable, log form, the outcome the Python hook must give
Scenario = tuple[str, list[list[object]], str, str | None, str, str]


def _repo(*files: tuple[str, str]) -> list[list[object]]:
    steps: list[list[object]] = [["init"], ["write", "repo/README.md", "readme\n"]]
    steps += [["write", f"repo/{path}", text] for path, text in files]
    return [*steps, ["commit"]]


def language_scenarios() -> list[Scenario]:
    """Return a clean, an advisory and a blocked Write in each tier-1 language."""
    scenarios: list[Scenario] = []
    for suffix, branchy in BRANCHY.items():
        target = f"<ROOT>/src/{FILE_NAMES.get(suffix, f'a.{suffix}')}"
        trivial, complex_write = (
            json.dumps(_write(target, TRIVIAL[suffix])),
            json.dumps(_write(target, branchy)),
        )
        scenarios += [
            (f"clean_{suffix}", _repo(), trivial, None, "log", "clean"),
            (
                f"advisory_{suffix}",
                _repo((CONFIG, THRESHOLD_ONE)),
                complex_write,
                None,
                "log",
                "advisory",
            ),
            (
                f"blocked_{suffix}",
                _repo((CONFIG, BLOCK_AT_ONE)),
                complex_write,
                None,
                "log",
                "blocked",
            ),
        ]
    return scenarios


def _pipe(
    name: str, steps: list[list[object]], document: object, outcome: str, **options: str
) -> Scenario:
    payload = document if isinstance(document, str) else json.dumps(document)
    return (name, steps, payload, options.get("gate"), options.get("log", "log"), outcome)


def pipe_scenarios() -> list[Scenario]:
    """Return the Python hook's pipe cases that an in-process lint keeps."""
    a_py = "<ROOT>/a.py"
    simple = _repo(("a.py", "x = 1\n"))
    twice = _repo(("a.py", "x = 1\nx = 1\n"))
    branch_new = "def f(x):\n    if x:\n        return 1\n    return x\n"
    one = _repo((CONFIG, THRESHOLD_ONE), ("a.py", "def f(x):\n    return x\n"))
    big: list[list[object]] = [["init"], ["repeat", "repo/big.py", "#", 1_000_001], ["commit"]]
    outside: list[list[object]] = [
        *simple,
        ["write", "outside.py", "z = 1\n"],
        ["symlink", "repo/link.py", "<SANDBOX>/outside.py"],
    ]
    return [
        _pipe("off_switch", simple, _write(a_py, "y = 2\n"), "none", gate="off"),
        _pipe("all_off", _repo((CONFIG, ALL_OFF)), _write(a_py), "skipped"),
        _pipe("clone_off", _repo((CONFIG, CLONE_OFF)), _write(a_py), "clean"),
        _pipe(
            "env_block",
            _repo((CONFIG, THRESHOLD_ONE)),
            _write(a_py, branch_new),
            "blocked",
            gate="block",
        ),
        _pipe("env_invalid", simple, _write(a_py, "y = 2\n"), "clean", gate="nope"),
        _pipe(
            "config_not_object", _repo((CONFIG, '{"erosion_gate": "block"}')), _write(a_py), "clean"
        ),
        _pipe("config_not_json", _repo((CONFIG, "{nope")), _write(a_py), "clean"),
        _pipe("malformed_not_json", simple, "{nope", "degraded"),
        _pipe("malformed_missing_id", simple, _without(_write(a_py), "tool_use_id"), "degraded"),
        _pipe("other_tool", simple, _document(tool_name="Read", tool_input={}), "skipped"),
        _pipe("symlink_out", outside, _write("<ROOT>/link.py"), "skipped"),
        _pipe("outside_any_repository", simple, _write("<SANDBOX>/elsewhere/a.py"), "skipped"),
        _pipe("new_directory", simple, _write("<ROOT>/new/deeper/a.py"), "clean"),
        _pipe("relative_path", simple, _write("b.py", cwd="<ROOT>"), "clean"),
        _pipe("not_tier1", simple, _write("<ROOT>/notes.md"), "skipped"),
        _pipe(
            "old_not_utf8",
            [["init"], ["bytes", "repo/a.py", "x = '\xff'\n"], ["commit"]],
            _write(a_py),
            "degraded",
        ),
        _pipe("old_too_large", big, _write("<ROOT>/big.py"), "degraded"),
        _pipe("old_unreadable", [*simple, ["chmod", "repo/a.py", 0]], _write(a_py), "degraded"),
        _pipe(
            "target_directory", [*simple, ["mkdir", "repo/d.py"]], _write("<ROOT>/d.py"), "skipped"
        ),
        _pipe("edit_not_in_file", simple, _edit(a_py, "nope", "y"), "skipped"),
        _pipe("edit_missing_file", simple, _edit("<ROOT>/b.py", "x", "y"), "skipped"),
        _pipe("edit_many", twice, _edit(a_py, "x = 1", "x = 2"), "skipped"),
        _pipe("edit_replace_all", twice, _edit(a_py, "x = 1", "x = 2", replace_all=True), "clean"),
        _pipe("edit_empty_old", simple, _edit(a_py, "", "y"), "skipped"),
        _pipe(
            "crlf_edit",
            _repo(("a.py", "a = 1\r\nb = 2\r\n")),
            _edit(a_py, "b = 2", "b = 3"),
            "clean",
        ),
        _pipe(
            "tie_edit", _repo(("a.py", "a = 1\r\nb = 2\n")), _edit(a_py, "b = 2", "b = 3"), "clean"
        ),
        _pipe("write_over_crlf", _repo(("a.py", "a = 1\r\n")), _write(a_py, "a = 2\n"), "clean"),
        _pipe(
            "branch_added",
            one,
            _edit(a_py, "    return x\n", "    if x:\n        return 1\n    return x\n"),
            "advisory",
        ),
        _pipe("fixture_path", _repo(), _write("<ROOT>/fixtures/a.py", branch_new), "any"),
        _pipe("unwritable_log", simple, _write(a_py, "y = 2\n"), "none", log="blocked"),
        _pipe("surrogate_content", simple, _write(a_py, "\ud800"), "degraded"),
        _pipe("nul_path", simple, _write("<ROOT>/a\x00b.py"), "degraded"),
        _pipe("deep_payload", simple, "[" * 9999 + "]" * 9999, "degraded"),
    ]


# The cache states. Each build fills its own cache with its own index: the reference a tags.db,
# the Rust build an agentless.db. "stale" commits a file after the index, so the tree moves on.
INDEX_FILE_ERRORS = 1
STALE_FILE = "STALE.md"
CACHE_GARBAGE = b"this file is not a SQLite database\n" * 128
# A function over the 40-token floor of a clone finding, by suffix: the head, the lines of one
# branch, and the tail, with NAME and K for the name and the branch number.
C_BRANCH = ["  if (x > K) { return K; }"]
C_TAIL = ["  return 0;", "}"]
TS_HEAD = "function NAME(x: number): number {"
CLONE_FORMS: dict[str, tuple[str, list[str], list[str]]] = {
    "py": ("def NAME(x):", ["    if x > K:", "        return K"], ["    return 0"]),
    "js": ("function NAME(x) {", C_BRANCH, C_TAIL),
    "ts": (TS_HEAD, C_BRANCH, C_TAIL),
    "tsx": (TS_HEAD, C_BRANCH, C_TAIL),
    "go": (
        "package main\n\nfunc NAME(x int) int {",
        ["\tif x > K {", "\t\treturn K", "\t}"],
        ["\treturn 0", "}"],
    ),
    "rs": (
        "fn NAME(x: i32) -> i32 {",
        ["    if x > K {", "        return K;", "    }"],
        ["    0", "}"],
    ),
    "java": (
        "class NAME {\n  int NAME(int x) {",
        ["    if (x > K) { return K; }"],
        ["    return 0;", "  }", "}"],
    ),
    "c": ("int NAME(int x) {", C_BRANCH, C_TAIL),
    "cpp": ("int NAME(int x) {", C_BRANCH, C_TAIL),
    "rb": ("def NAME(x)", ["  if x > K", "    return K", "  end"], ["  0", "end"]),
    "lua": (
        "function NAME(x)",
        ["  if x > K then", "    return K", "  end"],
        ["  return 0", "end"],
    ),
    "sh": ("NAME() {", ['  if [ "$1" -gt K ]; then', "    echo K", "  fi"], ["}"]),
}
CLONE_BRANCHES = 6
SOURCE_NAMES = {"java": "B.java"}
BROKEN_PY = "def broken(:\n    return 1\n"
TOKENIZE_REJECTS = (
    REPO / "tests" / "characterization" / "fixtures" / "complexity_py" / "tokenize_rejects.py"
)


def _clone(suffix: str, name: str) -> str:
    head, branch, tail = CLONE_FORMS[suffix]
    lines = [head.replace("NAME", name)]
    for number in range(CLONE_BRANCHES):
        lines += [line.replace("K", str(number)) for line in branch]
    return "\n".join([*lines, *tail]) + "\n"


def _states(scenario: Scenario) -> list[Scenario]:
    name, steps, payload, gate, log_form, outcome = scenario
    fresh: list[list[object]] = [*steps, ["index"]]
    stale: list[list[object]] = [*fresh, ["stale"]]
    return [
        (f"{name}_fresh", fresh, payload, gate, log_form, outcome),
        (f"{name}_stale", stale, payload, gate, log_form, outcome),
    ]


def store_scenarios() -> list[Scenario]:
    """Return the cases that read the stored rows: a clone in each tier-1 language, a changed
    source, the stored reasons of a file that does not measure, no rows, and a garbage cache."""
    scenarios: list[Scenario] = []
    for suffix in CLONE_FORMS:
        source = f"src/{SOURCE_NAMES.get(suffix, f'b.{suffix}')}"
        target = f"<ROOT>/src/{FILE_NAMES.get(suffix, f'a.{suffix}')}"
        steps: list[list[object]] = [*_repo((source, _clone(suffix, "kept"))), ["index"]]
        payload = json.dumps(_write(target, _clone(suffix, "added")))
        scenarios.append((f"clone_{suffix}", steps, payload, None, "log", "advisory"))
    changed: list[list[object]] = [
        *_repo(("src/b.py", _clone("py", "kept"))),
        ["index"],
        ["write", "repo/src/b.py", _clone("py", "moved")],
        ["commit"],
    ]
    copy = json.dumps(_write("<ROOT>/src/a.py", _clone("py", "added")))
    tokenizer = TOKENIZE_REJECTS.read_text(encoding="utf-8")
    plain = json.dumps(_write("<ROOT>/src/a.py", "x = 2\n"))
    return [
        *scenarios,
        ("clone_source_changed", changed, copy, None, "log", "advisory"),
        (
            "stored_does_not_parse",
            [*_repo(("broken.py", BROKEN_PY)), ["index"]],
            plain,
            None,
            "log",
            "any",
        ),
        (
            "stored_tokenizer_rejects",
            [*_repo(("rejects.py", tokenizer)), ["index"]],
            plain,
            None,
            "log",
            "any",
        ),
        ("no_rows", [*_repo(), ["index"]], plain, None, "log", "clean"),
        ("garbage_cache", [*_repo(), ["index"], ["cache_garbage"]], plain, None, "log", "clean"),
    ]


def states_fixture() -> Fixture:
    """Return every end-to-end scenario on a fresh and a stale cache, then the stored-row cases."""
    scenarios = [*language_scenarios(), *pipe_scenarios()]
    states = [state for scenario in scenarios for state in _states(scenario)]
    return {"states": [scenario_case(scenario) for scenario in [*states, *store_scenarios()]]}


def _git(root: Path, args: list[str], env: dict[str, str]) -> None:
    run_git(root, IDENTITY, *args, env=env)


def _put(sandbox: Path, step: list[object]) -> None:
    path = sandbox / str(step[1])
    path.parent.mkdir(parents=True, exist_ok=True)
    count = int(str(step[3])) if step[0] == "repeat" else 1
    encoding = "latin-1" if step[0] == "bytes" else "utf-8"
    path.write_bytes((str(step[2]) * count).encode(encoding))


def _git_step(sandbox: Path, step: list[object], env: dict[str, str]) -> None:
    root = sandbox / "repo"
    if step[0] == "init":
        root.mkdir()
        _git(root, ["init", "--quiet", "-b", "main"], env)
        return
    _git(root, ["add", "-A"], env)
    _git(root, ["commit", "--quiet", "--allow-empty", "-m", "fixture"], env)


def _apply(sandbox: Path, step: list[object], env: dict[str, str]) -> None:
    kind = step[0]
    if kind in ("init", "commit"):
        _git_step(sandbox, step, env)
    elif kind in ("write", "bytes", "repeat"):
        _put(sandbox, step)
    elif kind == "mkdir":
        (sandbox / str(step[1])).mkdir(parents=True)
    elif kind == "symlink":
        (sandbox / str(step[1])).symlink_to(str(step[2]).replace("<SANDBOX>", str(sandbox)))
    elif kind == "chmod":
        (sandbox / str(step[1])).chmod(int(str(step[2])))
    else:
        _store_step(sandbox, step, env)


def _store_step(sandbox: Path, step: list[object], env: dict[str, str]) -> None:
    root = sandbox / "repo"
    kind = step[0]
    if kind == "index":
        done = subprocess.run(
            [str(LINT_BINARY), "index", "--repo", str(root)],
            env=env,
            cwd=sandbox,
            capture_output=True,
            timeout=120,
            check=False,
        )
        if done.returncode not in (0, INDEX_FILE_ERRORS):
            message = f"refused: the index step exited {done.returncode}: {done.stderr!r}"
            raise SystemExit(message)
    elif kind == "stale":
        (root / STALE_FILE).write_text("stale\n", encoding="utf-8")
        _git(root, ["add", STALE_FILE], env)
        _git(root, ["commit", "--quiet", "-m", "fixture"], env)
    elif kind == "cache_garbage":
        (database,) = (sandbox / "home" / ".cache" / "agentless-mcp").glob("*/tags.db")
        database.write_bytes(CACHE_GARBAGE)
    else:
        message = f"refused: no scenario step {kind}"
        raise SystemExit(message)


@contextmanager
def _sandbox() -> Iterator[Path]:
    with tempfile.TemporaryDirectory() as scratch:
        sandbox = Path(scratch).resolve()
        try:
            yield sandbox
        finally:
            for path in sandbox.rglob("*"):
                if not path.is_symlink():
                    path.chmod(0o700)


def _placeholders(text: str, sandbox: Path) -> str:
    return text.replace(str(sandbox / "repo"), "<ROOT>").replace(str(sandbox), "<SANDBOX>")


def _normalized_record(log: Path, sandbox: Path, gate: str | None) -> str | None:
    if not log.exists():
        return None
    record = json.loads(log.read_text("utf-8"))
    record["ts"] = WALL_SECONDS
    record["latency_ms"] = 0.0
    if record["binary"] is not None:
        record["binary"] = "<BINARY>"
    if record["modes"] is not None:
        record["config_warnings"] = rust_config_warnings(sandbox / "repo", gate)
    return _placeholders(json.dumps(record) + "\n", sandbox)


def _scenario_env(sandbox: Path, log: Path, gate: str | None) -> dict[str, str]:
    env = {
        **hermetic_env(sandbox / "home"),
        GRAMMAR_OVERRIDE: os.environ[GRAMMAR_OVERRIDE],
        "AGENTLESS_MCP_NO_DOWNLOAD": "1",
        "AGENTLESS_MCP_NO_AUTO_WARM": "1",
        "AGENTLESS_EROSION_BIN": str(LINT_BINARY),
        hook.ENV_LOG: str(log),
    }
    if gate is not None:
        env[hook.ENV_GATE] = gate
    return env


def _log_path(sandbox: Path, form: str) -> Path:
    if form == "blocked":
        (sandbox / "blocker").write_text("a file, not a directory")
        return sandbox / "blocker" / "erosion.jsonl"
    return sandbox / "log" / "erosion.jsonl"


def _stderr(raw: bytes, sandbox: Path) -> tuple[str, int | None]:
    stderr = _placeholders(raw.decode("utf-8"), sandbox)
    # A Python traceback follows the first line; the Rust hook writes one error line (D6).
    if "Traceback" in stderr:
        return stderr.splitlines()[0], 1
    return stderr, None


def _run_scenario(sandbox: Path, scenario: Scenario) -> dict[str, object]:
    _, steps, payload, gate, log_form, _ = scenario
    log = _log_path(sandbox, log_form)
    env = _scenario_env(sandbox, log, gate)
    for step in steps:
        _apply(sandbox, step, env)
    placed = payload.replace("<ROOT>", str(sandbox / "repo")).replace("<SANDBOX>", str(sandbox))
    done = subprocess.run(
        [sys.executable, "-I", str(HOOK_PATH)],
        input=placed.encode("latin-1"),
        env=env,
        cwd=sandbox,
        capture_output=True,
        timeout=120,
        check=False,
    )
    stderr, stderr_lines = _stderr(done.stderr, sandbox)
    return {
        "code": done.returncode,
        "stdout": _placeholders(done.stdout.decode("utf-8"), sandbox),
        "stderr": stderr,
        "stderr_lines": stderr_lines,
        "record": _normalized_record(log, sandbox, gate),
    }


def scenario_case(scenario: Scenario) -> list[object]:
    """Return one scenario: the Python hook's code, stdout, stderr and normalized record."""
    name, steps, payload, gate, log_form, outcome = scenario
    with _sandbox() as sandbox:
        expected = _run_scenario(sandbox, scenario)
    record = expected["record"]
    found = json.loads(record)["outcome"] if isinstance(record, str) else "none"
    if outcome not in ("any", found):
        message = f"refused: scenario {name} gave {found}, not {outcome}"
        raise SystemExit(message)
    return [name, steps, payload, gate, log_form, expected]


def scenarios_fixture() -> Fixture:
    """Return the end-to-end scenarios."""
    scenarios = [*language_scenarios(), *pipe_scenarios()]
    return {"scenarios": [scenario_case(scenario) for scenario in scenarios]}


BUILDERS: dict[str, Callable[[], Fixture]] = {
    "text": text_fixture,
    "payload": payload_fixture,
    "edit": edit_fixture,
    "modes": modes_fixture,
    "target": target_fixture,
    "classify": classify_fixture,
    "scenarios": scenarios_fixture,
    "states": states_fixture,
}


def main() -> int:
    """Refuse a run that could reach the user's caches, then write or check the fixtures."""
    refuse_real_caches()
    if not os.environ.get(GRAMMAR_OVERRIDE):
        sys.exit(f"refused: set {GRAMMAR_OVERRIDE} to a scratch copy of the grammar libraries")
    if not LINT_BINARY.is_file():
        sys.exit(f"refused: {LINT_BINARY} is missing; run uv sync first")
    return write_or_check((__doc__ or "").splitlines()[0], FIXTURES, BUILDERS)


if __name__ == "__main__":
    raise SystemExit(main())
