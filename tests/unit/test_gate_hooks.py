"""The structural-first hooks gate broad discovery, not known-file search."""

import importlib.util
import io
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).parents[2]


def load_hook(name):
    """Load one standalone hook without putting contrib on ``sys.path``."""
    path = ROOT / "contrib" / "hooks" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.fixture
def hooks(tmp_path):
    check = load_hook("agentless_gate_check")
    mark = load_hook("agentless_gate_mark")
    marker_dir = tmp_path / "markers"
    check.MARKER_DIR = marker_dir
    mark.MARKER_DIR = marker_dir
    return check, mark


def call(module, monkeypatch, payload, function):
    monkeypatch.setattr(module.sys, "stdin", io.StringIO(json.dumps(payload)))
    return function()


def test_an_exact_file_grep_needs_no_structural_marker(hooks, monkeypatch, tmp_path):
    check, _ = hooks
    source = tmp_path / "app.py"
    source.write_text("VALUE = 1\n")
    payload = {
        "session_id": "known-file",
        "cwd": str(tmp_path),
        "tool_name": "Grep",
        "tool_input": {"pattern": "VALUE", "path": "app.py"},
    }

    assert call(check, monkeypatch, payload, check.decide) == check.ALLOW


@pytest.mark.parametrize(
    "case",
    [
        ("Grep", {"pattern": "VALUE"}, "broad_grep_before_structure"),
        ("Grep", {"pattern": "VALUE", "path": "src"}, "broad_grep_before_structure"),
        ("Glob", {"pattern": "**/*.py"}, "glob_before_structure"),
    ],
)
def test_broad_discovery_is_denied_before_localization(hooks, monkeypatch, tmp_path, case):
    check, _ = hooks
    tool_name, tool_input, reason = case
    (tmp_path / "src").mkdir()
    log = tmp_path / "gate.jsonl"
    monkeypatch.setenv(check.LOG_ENV, str(log))
    payload = {
        "session_id": "broad",
        "cwd": str(tmp_path),
        "tool_name": tool_name,
        "tool_input": tool_input,
    }

    assert call(check, monkeypatch, payload, check.decide) == check.DENY
    assert json.loads(log.read_text())["reason"] == reason


@pytest.mark.parametrize(
    ("tool_name", "tool_input", "reason"),
    [
        ("mcp__agentless__orient", {"operation": "map"}, "orient.map"),
        ("mcp__agentless__orient", {"operation": "path"}, "orient.path"),
        ("mcp__agentless__read", {"operation": "slice"}, "read.slice"),
        ("mcp__agentless__read_slice", {}, "read_slice"),
        ("mcp__agentless__symbols", {"operation": "find"}, "symbols.find"),
        ("mcp__agentless__symbols", {"operation": "overview"}, "symbols.overview"),
        ("mcp__agentless__symbols", {"operation": "expand"}, "symbols.expand"),
        ("mcp__agentless__symbols", {"operation": "explain"}, "symbols.explain"),
        ("mcp__agentless__find_referencing_symbols", {}, "find_referencing_symbols"),
        ("mcp__agentless__repo_map", {}, "repo_map"),
        ("mcp__agentless__get_symbols_overview", {}, "get_symbols_overview"),
        ("mcp__agentless__expand_symbols", {}, "expand_symbols"),
        ("mcp__agentless__find_symbol", {}, "find_symbol"),
        ("mcp__agentless__explain_symbol", {}, "explain_symbol"),
    ],
)
def test_localizing_operations_unlock_broad_search(
    hooks, monkeypatch, tool_name, tool_input, reason
):
    check, mark = hooks
    structural = {
        "session_id": "localized",
        "tool_name": tool_name,
        "tool_input": tool_input,
    }
    broad = {
        "session_id": "localized",
        "tool_name": "Grep",
        "tool_input": {"pattern": "VALUE"},
    }

    call(mark, monkeypatch, structural, mark.mark)

    marker = mark.marker_path("localized")
    assert marker.parent == mark.MARKER_DIR
    assert "localized" not in marker.name
    assert json.loads(marker.read_text())["reason"] == reason
    assert call(check, monkeypatch, broad, check.decide) == check.ALLOW


@pytest.mark.parametrize(
    ("tool_name", "tool_input"),
    [
        ("mcp__agentless__capabilities", {}),
        ("mcp__agentless__orient", {"operation": "health"}),
        ("mcp__agentless__orient", {"operation": "communities"}),
        ("mcp__agentless__orient", {"operation": "cycles"}),
        ("mcp__agentless__orient", {"operation": "diagram"}),
        ("mcp__agentless__read", {"operation": "dir"}),
        ("mcp__agentless__symbols", {"operation": "locate"}),
        ("mcp__agentless__list_dir", {}),
        ("mcp__agentless__resolve_locations", {}),
        ("mcp__agentless__analyze_structure", {"operation": "health"}),
    ],
)
def test_non_localizing_calls_do_not_unlock_broad_search(hooks, monkeypatch, tool_name, tool_input):
    check, mark = hooks
    payload = {
        "session_id": "diagnostic",
        "tool_name": tool_name,
        "tool_input": tool_input,
    }
    broad = {
        "session_id": "diagnostic",
        "tool_name": "Grep",
        "tool_input": {"pattern": "VALUE"},
    }

    call(mark, monkeypatch, payload, mark.mark)

    assert not mark.marker_path("diagnostic").exists()
    assert call(check, monkeypatch, broad, check.decide) == check.DENY


def test_naming_a_file_and_a_range_unlocks_like_an_exact_file_grep(hooks, monkeypatch, tmp_path):
    """The two halves of the gate agree on what counts as already localized.

    The check hook lets an exact-file Grep through because the caller has
    localized that search itself. `read(slice)` names a file *and* a line
    range, so refusing it while allowing the weaker Grep was a contradiction.
    """
    check, mark = hooks
    target = tmp_path / "app.py"
    target.write_text("x = 1\n", encoding="utf-8")
    broad = {"session_id": "sliced", "tool_name": "Grep", "tool_input": {"pattern": "VALUE"}}
    scoped = {
        "session_id": "sliced",
        "tool_name": "Grep",
        "tool_input": {"pattern": "VALUE", "path": str(target)},
    }

    # Before any unlock the scoped Grep is allowed and the broad one is not.
    assert call(check, monkeypatch, scoped, check.decide) == check.ALLOW
    assert call(check, monkeypatch, broad, check.decide) == check.DENY

    # A read(slice) is at least as much evidence, so it opens the gate.
    call(
        mark,
        monkeypatch,
        {
            "session_id": "sliced",
            "tool_name": "mcp__agentless__read",
            "tool_input": {"operation": "slice", "path": "app.py", "lines": [[1, 5]]},
        },
        mark.mark,
    )
    assert call(check, monkeypatch, broad, check.decide) == check.ALLOW


def test_a_listing_is_how_you_look_not_proof_you_found(hooks, monkeypatch):
    """`read(dir)` stays out: it is discovery, not localization."""
    check, mark = hooks
    broad = {"session_id": "listing", "tool_name": "Grep", "tool_input": {"pattern": "VALUE"}}

    call(
        mark,
        monkeypatch,
        {
            "session_id": "listing",
            "tool_name": "mcp__agentless__read",
            "tool_input": {"operation": "dir"},
        },
        mark.mark,
    )

    assert not mark.marker_path("listing").exists()
    assert call(check, monkeypatch, broad, check.decide) == check.DENY


def test_the_log_records_which_operation_was_refused(hooks, monkeypatch, tmp_path):
    """A refusal that names only the tool cannot be analysed afterwards.

    `read(slice)` and `read(dir)` are treated differently, so a log that kept
    only `read` could not say which one a session was denied.
    """
    _, mark = hooks
    log = tmp_path / "gate.jsonl"
    monkeypatch.setenv(mark.LOG_ENV, str(log))

    call(
        mark,
        monkeypatch,
        {
            "session_id": "logged",
            "tool_name": "mcp__agentless__read",
            "tool_input": {"operation": "dir"},
        },
        mark.mark,
    )
    call(
        mark,
        monkeypatch,
        {
            "session_id": "logged",
            "tool_name": "mcp__agentless__read",
            "tool_input": {"operation": "slice"},
        },
        mark.mark,
    )

    rows = [json.loads(line) for line in log.read_text().splitlines()]
    assert [(r["event"], r["operation"]) for r in rows] == [
        ("agentless_call_ignored", "dir"),
        ("structural_call", "slice"),
    ]


def test_sessions_do_not_unlock_each_other(hooks, monkeypatch):
    check, mark = hooks
    call(
        mark,
        monkeypatch,
        {
            "session_id": "first",
            "tool_name": "mcp__agentless__orient",
            "tool_input": {"operation": "map"},
        },
        mark.mark,
    )

    second = {
        "session_id": "second",
        "tool_name": "Glob",
        "tool_input": {"pattern": "**/*.py"},
    }
    assert call(check, monkeypatch, second, check.decide) == check.DENY


def test_missing_session_and_malformed_input_fail_open(hooks, monkeypatch, tmp_path):
    check, _ = hooks
    log = tmp_path / "gate.jsonl"
    monkeypatch.setenv(check.LOG_ENV, str(log))
    missing = {"tool_name": "Glob", "tool_input": {"pattern": "**/*.py"}}

    assert call(check, monkeypatch, missing, check.main) == check.ALLOW

    monkeypatch.setattr(check.sys, "stdin", io.StringIO("{"))
    assert check.main() == check.ALLOW
    reasons = [json.loads(line)["reason"] for line in log.read_text().splitlines()]
    assert reasons == ["no_session_id", "malformed_payload"]


@pytest.mark.parametrize(
    "payload",
    [
        [],
        {"session_id": "broken", "tool_name": "Grep"},
        {"session_id": "broken", "tool_name": "Grep", "tool_input": {}},
        {
            "session_id": "broken",
            "tool_name": "Grep",
            "tool_input": {"pattern": 7},
        },
    ],
)
def test_structurally_malformed_payloads_fail_open(hooks, monkeypatch, payload):
    check, _ = hooks

    assert call(check, monkeypatch, payload, check.decide) == check.ALLOW


def test_a_tool_this_gate_does_not_govern_is_named_as_such(hooks, monkeypatch, tmp_path):
    """A too-wide hook match and an unreadable Grep are different events."""
    check, _ = hooks
    log = tmp_path / "gate.jsonl"
    monkeypatch.setenv(check.LOG_ENV, str(log))
    ungoverned = {"session_id": "wide", "tool_name": "Read", "tool_input": {"file_path": "a.py"}}
    unreadable = {"session_id": "wide", "tool_name": "Grep", "tool_input": {}}

    assert call(check, monkeypatch, ungoverned, check.decide) == check.ALLOW
    assert call(check, monkeypatch, unreadable, check.decide) == check.ALLOW

    reasons = [json.loads(line)["reason"] for line in log.read_text().splitlines()]
    assert reasons == ["not_a_search_tool", "malformed_payload"]


def test_unavailable_marker_storage_fails_open(hooks, monkeypatch):
    check, _ = hooks
    monkeypatch.setattr(check, "_marker_storage_ready", lambda: False)
    payload = {
        "session_id": "no-storage",
        "tool_name": "Glob",
        "tool_input": {"pattern": "**/*.py"},
    }

    assert call(check, monkeypatch, payload, check.decide) == check.ALLOW
