"""The per-edit erosion gate: proposed text, modes, lint output, the reply, and failing open."""

import importlib.util
import io
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

import pytest

from agentless_mcp.application.lint_service import CHECK_NAMES, PAIR_CHECK_NAMES
from agentless_mcp.core import cache, erosion, grammars, patchlint, projectconfig, selfrestart
from agentless_mcp.core.extractor import TreeSitterExtractor
from agentless_mcp.prompts import MESSAGES
from agentless_mcp.util.fslimits import DEFAULT_MAX_FILE_BYTES

ROOT = Path(__file__).parents[2]
HOOK = ROOT / "contrib" / "hooks" / "agentless_erosion_gate.py"
SETTINGS = ROOT / "contrib" / "hooks" / "settings-example.json"
CLI = Path(sys.executable).parent / "agentless-mcp"
GIT_DIR = str(Path(shutil.which("git") or "git").parent)

PINNED_ENV = (
    grammars.ENV_CACHE_DIR,
    grammars.ENV_NO_AUTO_WARM,
    cache.ENV_NO_AUTO_INDEX,
    selfrestart.ENV_NO_AUTO_RESTART,
    cache.ENV_MAX_CACHE_BYTES,
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_NOSYSTEM",
)
HOOK_ENV = ("AGENTLESS_EROSION_GATE", "AGENTLESS_EROSION_BIN", "AGENTLESS_EROSION_LOG")

BRANCHES = "".join(f"    if x == {i}:\n        return {i}\n" for i in range(11))
SOURCE = f"def big(x):\n{BRANCHES}    return -1\n\n\ndef small(y):\n    return y\n"
ADD_BRANCH = ("    return -1\n", "    if x == 99:\n        return 99\n    return -1\n")

RELEASED_USAGE = (
    "usage: agentless-mcp lint [-h] (--candidates PATH | --diff FILE) [--repo PATH]\n"
    "                          [--json] [--no-cache]\n"
    "agentless-mcp lint: error: one of the arguments --candidates --diff is required\n"
)

S0_EDITS = [
    pytest.param(
        b"line one\r\nline two\r\nline three\r\nline four\r\n",
        "line two\nline three",
        "line 2\nline 3",
        b"line one\r\nline 2\r\nline 3\r\nline four\r\n",
        id="crlf.txt",
    ),
    pytest.param(
        b"one\r\ntwo\nthree\nfour\n", "two\nthree", "2\n3", b"one\n2\n3\nfour\n", id="mixa.txt"
    ),
    pytest.param(
        b"one\ntwo\r\nthree\r\nfour\r\n",
        "two\nthree",
        "2\n3",
        b"one\r\n2\r\n3\r\nfour\r\n",
        id="mixb.txt",
    ),
]

ABSENT = object()
CONTRACT_BLOCKS = {"complexity_delta": "advisory", "contract_drift": "block"}
COMPLEXITY_BLOCKS = {"complexity_delta": "block", "contract_drift": "advisory"}


def load_hook():
    spec = importlib.util.spec_from_file_location("agentless_erosion_gate", HOOK)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # A dataclass resolves its string annotations through sys.modules while the class is built.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


hook = load_hook()

STUB = """#!{python}
import json
import os
import sys
import time
from pathlib import Path

calls = Path(os.environ["STUB_CALLS"])
argv = sys.argv[1:]
after = Path(argv[argv.index("--after") + 1]) if "--after" in argv else None
entry = {{"argv": argv, "pid": os.getpid(), "after": after.read_bytes().hex() if after else None}}
with calls.open("a") as handle:
    handle.write(json.dumps(entry) + "\\n")
responses = json.loads(Path(os.environ["STUB_SPEC"]).read_text())
response = responses[min(len(calls.read_text().splitlines()), len(responses)) - 1]
time.sleep(response.get("sleep", 0))
sys.stdout.write(response.get("stdout", ""))
sys.stderr.write(response.get("stderr", ""))
sys.exit(response.get("code", 0))
"""


def lint_stdout(*findings, observed=ABSENT, rel="pkg/app.py"):
    candidate = {"id": rel, "findings": list(findings)}
    if observed is not ABSENT:
        candidate["observed"] = observed
    receipt = {"repo": "/repo", "head": "0000000f", "tree": "1111111f", "dirty": 0}
    return json.dumps(
        {"receipt": receipt, "notice": "repository data below", "candidates": [candidate]}
    )


def complexity_finding(*, unit="big", line=1, cc_old=12, cc_new=13, rel="pkg/app.py"):
    return {
        "check": "complexity_delta",
        "severity": "warning",
        "message": f"{unit}: complexity {cc_old} -> {cc_new}. Put the new branch in a function.",
        "path": rel,
        "line": line,
        "location": f"{rel}:{line}",
        "evidence": f"cc {cc_old} -> {cc_new}; threshold 10",
        "detail": {
            "unit": unit,
            "match": "name",
            "cc_old": cc_old,
            "cc_new": cc_new,
            "sloc_old": 20,
            "sloc_new": 22,
            "threshold": 10,
        },
    }


def contract_finding(*, line=9, rel="pkg/app.py"):
    return {
        "check": "contract_drift",
        "severity": "warning",
        "message": "parse: a silent default on foreign data.",
        "path": rel,
        "line": line,
        "location": f"{rel}:{line}",
        "evidence": 'data.get("items", [])',
        "detail": {
            "rule": "get-default-empty",
            "unit": "parse",
            "capture": 'data.get("items", [])',
        },
    }


def contract_row(reason):
    return {
        "check": "contract_drift",
        "severity": "not_checked",
        "message": "not checked: contract_drift did not run on this file.",
        "path": "",
        "line": 0,
        "location": "(repository)",
        "evidence": reason,
        "detail": {"reason": reason},
    }


def finding(**fields):
    base = {
        "check": "complexity_delta",
        "severity": "warning",
        "message": "m",
        "path": "pkg/app.py",
        "line": 1,
        "evidence": "e",
        "detail": None,
    }
    return hook.LintFinding(**{**base, **fields})


@dataclass
class Gate:
    tmp_path: Path
    root: Path
    env: dict
    log: Path
    calls: Path
    spec: Path
    temp_dir: Path

    def respond(self, *responses):
        self.spec.write_text(json.dumps(list(responses)), encoding="utf-8")

    def payload(self, tool_name, rel, fields, tool_use_id="toolu_test1"):
        return {
            "session_id": "session-1",
            "transcript_path": str(self.tmp_path / "transcript.jsonl"),
            "cwd": str(self.root),
            "prompt_id": "prompt-1",
            "permission_mode": "acceptEdits",
            "effort": {"level": "high"},
            "hook_event_name": "PreToolUse",
            "tool_name": tool_name,
            "tool_input": {"file_path": str(self.root / rel), **fields},
            "tool_use_id": tool_use_id,
        }

    def edit(self, rel="pkg/app.py", old=ADD_BRANCH[0], new=ADD_BRANCH[1], replace_all=False):
        fields = {"old_string": old, "new_string": new, "replace_all": replace_all}
        return self.payload("Edit", rel, fields)

    def write(self, rel, content):
        return self.payload("Write", rel, {"content": content})

    def run(self, payload, **env):
        data = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
        return subprocess.run(
            [sys.executable, str(HOOK)],
            input=data,
            capture_output=True,
            env={**self.env, **env},
            timeout=60,
            check=False,
        )

    def records(self):
        return [json.loads(line) for line in self.log.read_text(encoding="utf-8").splitlines()]

    def stub_calls(self):
        if not self.calls.exists():
            return []
        return [json.loads(line) for line in self.calls.read_text(encoding="utf-8").splitlines()]

    def temp_files(self):
        return sorted(self.temp_dir.glob("agentless-erosion-*"))


@pytest.fixture
def gate(tmp_path, make_git_repo):
    root = make_git_repo({"pkg/app.py": SOURCE, "notes.txt": "notes\n"})
    temp_dir = tmp_path / "tmp"
    home = tmp_path / "home"
    stub = tmp_path / "bin" / "agentless-mcp"
    for directory in (temp_dir, home, stub.parent):
        directory.mkdir()
    stub.write_text(STUB.format(python=sys.executable), encoding="utf-8")
    stub.chmod(0o755)
    env = {name: os.environ[name] for name in PINNED_ENV if name in os.environ}
    env.update(
        PATH=GIT_DIR,
        HOME=str(home),
        TMPDIR=str(temp_dir),
        XDG_CACHE_HOME=str(tmp_path / "xdg"),
        AGENTLESS_EROSION_BIN=str(stub),
        AGENTLESS_EROSION_LOG=str(tmp_path / "gate.jsonl"),
        STUB_CALLS=str(tmp_path / "calls.jsonl"),
        STUB_SPEC=str(tmp_path / "spec.json"),
    )
    built = Gate(
        tmp_path=tmp_path,
        root=root,
        env=env,
        log=tmp_path / "gate.jsonl",
        calls=tmp_path / "calls.jsonl",
        spec=tmp_path / "spec.json",
        temp_dir=temp_dir,
    )
    built.respond({"stdout": lint_stdout()})
    yield built
    assert built.temp_files() == []


@pytest.fixture
def restore_sigterm():
    previous = signal.getsignal(signal.SIGTERM)
    yield
    signal.signal(signal.SIGTERM, previous)


class TestProposedText:
    @pytest.mark.parametrize(("before", "old", "new", "written"), S0_EDITS)
    def test_the_crlf_rule_reproduces_the_bytes_claude_code_wrote(self, before, old, new, written):
        proposal = hook.edited_text(before.decode("utf-8"), hook.EditInput(old, new, False))

        assert proposal.text.encode("utf-8") == written

    def test_replace_all_replaces_every_occurrence_and_one_edit_replaces_one(self):
        every = hook.edited_text("a = 1\nb = a\n", hook.EditInput("a", "c", True))
        once = hook.edited_text("a = 1\nb = 2\n", hook.EditInput("b = 2", "b = 3", False))

        assert every.text == "c = 1\nb = c\n"
        assert once.text == "a = 1\nb = 3\n"

    @pytest.mark.parametrize(
        ("old", "new", "reason"),
        [
            ("", "x", "empty old_string"),
            ("a", "a", "equals new_string"),
            ("zzz", "y", "not in the file"),
            ("x", "y", "occurs 2 times"),
        ],
    )
    def test_an_edit_claude_code_fails_is_skipped(self, old, new, reason):
        with pytest.raises(hook._HaltError) as halted:
            hook.edited_text("a x\nx b\n", hook.EditInput(old, new, False))

        assert halted.value.outcome == "skipped"
        assert reason in halted.value.reason

    def test_a_tie_gets_lf_and_is_recorded_while_no_line_ending_is_lf(self):
        tie = hook.edited_text("one\r\ntwo\n", hook.EditInput("two", "2", False))
        bare = hook.edited_text("x = 1", hook.EditInput("x = 1", "x = 1\ny = 2", False))

        assert (tie.text, tie.eol) == ("one\n2\n", "tie")
        assert (bare.text, bare.eol) == ("x = 1\ny = 2", "lf")


class TestPayload:
    def base(self, **tool_input):
        return {
            "session_id": "s",
            "transcript_path": "/t.jsonl",
            "tool_use_id": "toolu_1",
            "tool_name": "Edit",
            "cwd": "/repo",
            "tool_input": {"file_path": "/repo/a.py", "old_string": "a", "new_string": "b"}
            | tool_input,
        }

    def test_a_missing_replace_all_reads_as_false(self):
        payload = hook.parse_payload(self.base())

        assert payload.change == hook.EditInput("a", "b", False)

    def test_a_relative_file_path_joins_cwd(self):
        payload = hook.parse_payload(self.base(file_path="pkg/a.py"))

        assert payload.file_path == Path("/repo/pkg/a.py")

    @pytest.mark.parametrize(
        ("change", "reason"),
        [
            ({"replace_all": "yes"}, "tool_input.replace_all is not a bool"),
            ({"old_string": 3}, "tool_input.old_string is not a str"),
            ({"file_path": None}, "tool_input.file_path is not a str"),
        ],
    )
    def test_a_mistyped_tool_input_field_is_malformed(self, change, reason):
        with pytest.raises(hook._ShapeError, match=reason):
            hook.parse_payload(self.base(**change))

    @pytest.mark.parametrize("key", ["session_id", "transcript_path", "tool_use_id", "tool_input"])
    def test_a_missing_payload_field_is_malformed(self, key):
        document = self.base()
        del document[key]

        with pytest.raises(hook._ShapeError, match=f"payload.{key} is missing"):
            hook.parse_payload(document)

    def test_another_tool_is_skipped_not_malformed(self):
        with pytest.raises(hook._HaltError) as halted:
            hook.parse_payload({"tool_name": "Read", "tool_input": {}})

        assert halted.value.outcome == "skipped"


class TestModes:
    def test_the_repository_entry_wins_over_the_environment_and_the_default(self):
        modes, warnings = hook.resolve_modes({"complexity_delta": "block"}, "advisory")

        assert modes == {"complexity_delta": "block", "contract_drift": "advisory"}
        assert warnings == []

    def test_the_environment_sets_the_default_for_checks_the_repository_leaves_unset(self):
        modes, _ = hook.resolve_modes({"contract_drift": "off"}, "block")

        assert modes == {"complexity_delta": "block", "contract_drift": "off"}

    def test_an_invalid_environment_value_falls_back_to_advisory_with_a_warning(self):
        modes, warnings = hook.resolve_modes({}, "enforce")

        assert set(modes.values()) == {"advisory"}
        assert "AGENTLESS_EROSION_GATE='enforce'" in warnings[0]

    def test_an_invalid_repository_entry_is_dropped_with_a_warning(self):
        entries, warnings = hook.repo_modes(
            {"erosion_gate": {"complexity_delta": "strict", "contract_drift": "block"}}
        )

        assert entries == {"contract_drift": "block"}
        assert "erosion_gate.complexity_delta is 'strict'" in warnings[0]

    def test_an_erosion_gate_that_is_not_an_object_is_ignored_with_a_warning(self):
        assert hook.repo_modes({"erosion_gate": "block"}) == (
            {},
            ["erosion_gate is not an object and was ignored"],
        )
        assert hook.repo_modes(None) == ({}, [])


class TestLintOutput:
    def test_the_real_cli_shape_parses(self):
        output = hook.parse_lint_output(
            lint_stdout(complexity_finding()).encode(), ["complexity_delta"]
        )

        assert output.findings[0].detail["unit"] == "big"
        assert output.observed is None

    def test_observed_passes_through_unchanged(self):
        observed = {"units_over_threshold": [], "future_list": [1]}

        output = hook.parse_lint_output(
            lint_stdout(observed=observed).encode(), ["complexity_delta"]
        )

        assert output.observed == observed

    @pytest.mark.parametrize(
        ("stdout", "reason"),
        [
            (b"not json", "not JSON"),
            (b"[]", "not a JSON object"),
            (b'{"receipt": {}}', "output.candidates is missing"),
            (b'{"candidates": [{"findings": []}, {"findings": []}]}', "holds 2 items"),
            (b'{"candidates": [{"findings": {}}]}', "candidate.findings is not a list"),
            (lint_stdout(observed=[]).encode(), "candidate.observed is not an object"),
        ],
    )
    def test_another_document_shape_is_refused(self, stdout, reason):
        with pytest.raises(hook._ShapeError, match=reason):
            hook.parse_lint_output(stdout, ["complexity_delta"])

    @pytest.mark.parametrize(
        ("change", "reason"),
        [
            ({"line": True}, "line is not a int"),
            ({"message": None}, "message is not a str"),
            ({"severity": "error"}, "severity 'error'"),
            ({"check": "arity"}, "check 'arity' was not requested"),
            ({"check": "coverage"}, "severity 'warning' is not valid for coverage"),
            ({"detail": []}, "detail is not an object"),
        ],
    )
    def test_another_finding_shape_is_refused(self, change, reason):
        stdout = lint_stdout(complexity_finding() | change).encode()

        with pytest.raises(hook._ShapeError, match=reason):
            hook.parse_lint_output(stdout, ["complexity_delta"])


class TestClassification:
    def test_the_no_boundary_row_is_the_designed_off_state(self):
        row = finding(
            check="contract_drift",
            severity="not_checked",
            detail=contract_row("no_boundary_globs")["detail"],
        )

        verdict = hook.classify(hook.LintOutput((row,), None), CONTRACT_BLOCKS)

        assert (verdict.outcome, verdict.off_by_config, verdict.gaps) == (
            "clean",
            ("contract_drift",),
            (),
        )

    def test_any_other_not_checked_row_is_a_gap_and_never_clean(self):
        row = finding(
            check="contract_drift",
            severity="not_checked",
            message="no rules for c",
            detail={"reason": "no_rules_for_language", "language": "c"},
        )

        verdict = hook.classify(hook.LintOutput((row,), None), CONTRACT_BLOCKS)

        assert (verdict.outcome, verdict.reason) == ("degraded", "no rules for c")

    def test_a_finding_beside_a_gap_keeps_the_gap(self):
        gap = finding(check="coverage", severity="not_checked", message="not read")
        warning = finding()

        verdict = hook.classify(hook.LintOutput((gap, warning), None), CONTRACT_BLOCKS)

        assert verdict.outcome == "advisory"
        assert verdict.gaps == (gap,)

    def test_a_finding_from_a_blocking_check_blocks(self):
        verdict = hook.classify(
            hook.LintOutput((finding(), finding(check="contract_drift")), None), CONTRACT_BLOCKS
        )

        assert verdict.outcome == "blocked"
        assert [f.check for f in verdict.blocking] == ["contract_drift"]


class TestFindingsText:
    def test_blocking_findings_come_first_and_advisory_ones_under_their_label(self):
        text = hook.findings_text(
            [finding(message="grew", line=3)],
            [finding(message="silent", line=9)],
            COMPLEXITY_BLOCKS,
        )

        assert text.splitlines() == [
            hook.BLOCK_HEADER,
            "pkg/app.py:3: grew",
            hook.ADVISORY_LABEL,
            "pkg/app.py:9: silent",
            (
                "Erosion gate mode: complexity_delta block, contract_drift advisory. Change it "
                "with the erosion_gate key in .agentless-mcp.json or the AGENTLESS_EROSION_GATE "
                "variable (off, advisory, block)."
            ),
        ]

    def test_at_most_15_findings_are_listed_and_the_rest_are_counted(self):
        many = [finding(line=n) for n in range(1, 21)]

        lines = hook.findings_text([], many, COMPLEXITY_BLOCKS).splitlines()

        assert lines[0] == hook.ADVISORY_HEADER
        assert len(lines) == 1 + 15 + 2
        assert lines[-2] == "...and 5 more findings"


class TestPipe:
    def test_advisory_prints_the_frozen_json_form_and_exits_0(self, gate):
        gate.respond({"stdout": lint_stdout(complexity_finding())})

        result = gate.run(gate.edit())

        assert result.returncode == 0
        assert result.stderr == b""
        assert json.loads(result.stdout) == {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": "\n".join(
                    [
                        hook.ADVISORY_HEADER,
                        "pkg/app.py:1: big: complexity 12 -> 13. Put the new branch in a function.",
                        hook._mode_line(dict.fromkeys(hook.KNOWN_CHECKS, "advisory")),
                    ]
                ),
            }
        }
        assert b"permissionDecision" not in result.stdout
        [record] = gate.records()
        assert (record["outcome"], record["tool_use_id"]) == ("advisory", "toolu_test1")
        assert record["findings"][0]["detail"]["cc_new"] == 13

    def test_block_exits_2_with_the_findings_on_stderr_and_nothing_on_stdout(self, gate):
        config = {"erosion_gate": {"complexity_delta": "block"}}
        (gate.root / ".agentless-mcp.json").write_text(json.dumps(config), encoding="utf-8")
        gate.respond({"stdout": lint_stdout(complexity_finding(), contract_finding())})

        result = gate.run(gate.edit())

        assert result.returncode == 2
        assert result.stdout == b""
        lines = result.stderr.decode().splitlines()
        assert lines[:2] == [hook.BLOCK_HEADER, lines[1]]
        assert lines[1].startswith("pkg/app.py:1: big")
        assert lines[2] == hook.ADVISORY_LABEL
        assert lines[3].startswith("pkg/app.py:9: parse")
        assert gate.records()[0]["outcome"] == "blocked"

    def test_the_machine_wide_off_switch_does_no_work(self, gate):
        result = gate.run(gate.edit(), AGENTLESS_EROSION_GATE="off")

        assert (result.returncode, result.stdout, result.stderr) == (0, b"", b"")
        assert not gate.log.exists()
        assert gate.stub_calls() == []

    def test_every_check_off_in_the_repository_skips_without_lint(self, gate):
        config = {"erosion_gate": {"complexity_delta": "off", "contract_drift": "off"}}
        (gate.root / ".agentless-mcp.json").write_text(json.dumps(config), encoding="utf-8")

        result = gate.run(gate.edit())

        assert (result.returncode, result.stdout) == (0, b"")
        assert gate.records()[0]["reason"] == "every check is off"
        assert gate.stub_calls() == []

    def test_a_clean_call_logs_observed_as_lint_sent_it(self, gate):
        observed = {
            "units_over_threshold": [{"unit": "big", "line": 1, "cc": 12, "sloc": 24}],
            "contract_keys": None,
            "units_changed": [],
        }
        gate.respond({"stdout": lint_stdout(observed=observed)})

        result = gate.run(gate.edit())

        assert (result.returncode, result.stdout) == (0, b"")
        [record] = gate.records()
        assert (record["outcome"], record["observed"]) == ("clean", observed)

    def test_a_clean_call_without_observed_logs_null(self, gate):
        result = gate.run(gate.edit())

        assert result.returncode == 0
        assert gate.records()[0]["observed"] is None
        assert gate.records()[0]["outcome"] == "clean"

    @pytest.mark.parametrize(
        "payload",
        [b"not json", b"[1]", b'{"tool_name": "Edit", "tool_input": {}}', b"\xff\xfe"],
    )
    def test_a_malformed_payload_fails_open(self, gate, payload):
        result = gate.run(payload)

        assert (result.returncode, result.stdout, result.stderr) == (0, b"", b"")
        [record] = gate.records()
        assert record["outcome"] == "degraded"
        assert record["reason"].startswith("malformed payload")

    def test_a_payload_over_4_mib_is_degraded(self, gate):
        result = gate.run(gate.write("pkg/app.py", "x" * (4 * 1024 * 1024)))

        assert result.returncode == 0
        assert gate.records()[0]["reason"] == "malformed payload: over 4 MiB"

    def test_a_release_without_the_hook_form_is_degraded(self, gate):
        gate.respond({"code": 2, "stderr": RELEASED_USAGE})

        result = gate.run(gate.edit())

        assert (result.returncode, result.stdout) == (0, b"")
        assert gate.records()[0]["reason"] == "agentless-mcp lacks the hook form"

    def test_a_check_the_binary_does_not_offer_is_dropped_and_lint_runs_again(self, gate):
        refusal = MESSAGES.lint_unknown_check.format(
            name="contract_drift", checks="complexity_delta, cycle_delta"
        )
        gate.respond(
            {"code": 2, "stderr": f"usage: agentless-mcp lint ...\nerror: {refusal}\n"},
            {"stdout": lint_stdout(complexity_finding())},
        )

        result = gate.run(gate.edit())

        assert result.returncode == 0
        assert (
            "big: complexity"
            in json.loads(result.stdout)["hookSpecificOutput"]["additionalContext"]
        )
        only = [call["argv"][call["argv"].index("--only") + 1] for call in gate.stub_calls()]
        assert only == ["complexity_delta,contract_drift", "complexity_delta"]
        record = gate.records()[0]
        assert record["dropped"] == [{"check": "contract_drift", "reason": "unknown to the binary"}]
        assert record["checks"] == ["complexity_delta"]

    def test_a_missing_binary_skips(self, gate):
        named = gate.run(gate.edit(), AGENTLESS_EROSION_BIN=str(gate.tmp_path / "absent"))
        on_path = gate.run(gate.edit(), AGENTLESS_EROSION_BIN="")

        assert shutil.which("agentless-mcp", path=GIT_DIR) is None
        assert (named.returncode, on_path.returncode) == (0, 0)
        reasons = [record["reason"] for record in gate.records()]
        assert reasons[0].startswith("AGENTLESS_EROSION_BIN does not name an executable file")
        assert reasons[1] == "agentless-mcp not found on PATH"
        assert gate.stub_calls() == []

    def test_a_symlink_out_of_the_repository_skips(self, gate):
        outside = gate.tmp_path / "outside.py"
        outside.write_text(SOURCE, encoding="utf-8")
        (gate.root / "pkg" / "link.py").symlink_to(outside)

        result = gate.run(gate.edit(rel="pkg/link.py"))

        assert result.returncode == 0
        assert gate.records()[0]["reason"] == "the resolved target is outside the repository"
        assert gate.stub_calls() == []

    def test_a_target_outside_any_repository_skips(self, gate):
        loose = gate.tmp_path / "loose"
        loose.mkdir()
        payload = gate.write("pkg/x.py", "x = 1\n")
        payload["tool_input"]["file_path"] = str(loose / "x.py")

        result = gate.run(payload)

        assert result.returncode == 0
        assert gate.records()[0]["reason"] == "not in a git repository"

    def test_a_write_into_a_new_directory_finds_the_repository(self, gate):
        result = gate.run(gate.write("new/deep/mod.py", "def f():\n    return 1\n"))

        assert result.returncode == 0
        [call] = gate.stub_calls()
        assert call["argv"][call["argv"].index("--path") + 1] == "new/deep/mod.py"
        record = gate.records()[0]
        assert (record["outcome"], record["old_sha256"]) == ("clean", None)

    def test_a_file_the_pair_checks_do_not_read_skips(self, gate):
        result = gate.run(gate.write("notes.txt", "more notes\n"))

        assert result.returncode == 0
        [record] = gate.records()
        assert record["reason"] == "not a tier-1 source file: notes.txt"
        assert record["path"] == str((gate.root / "notes.txt").resolve())
        assert gate.stub_calls() == []

    @pytest.mark.parametrize(
        ("content", "reason"),
        [
            pytest.param(
                b"#" * 1_000_001, "the old text is over 1,000,000 bytes", id="over-the-cap"
            ),
            pytest.param(b"x = '\xff'\n", "the old text is not UTF-8: byte 5", id="not-utf8"),
        ],
    )
    def test_an_old_text_lint_cannot_read_is_degraded(self, gate, content, reason):
        (gate.root / "pkg" / "app.py").write_bytes(content)

        result = gate.run(gate.write("pkg/app.py", "x = 1\n"))

        assert result.returncode == 0
        assert gate.records()[0]["reason"] == reason

    def test_an_edit_claude_code_would_fail_skips_without_lint(self, gate):
        result = gate.run(gate.edit(old="not in the file", new="x"))

        assert result.returncode == 0
        assert gate.records()[0]["outcome"] == "skipped"
        assert gate.stub_calls() == []

    @pytest.mark.parametrize(("before", "old", "new", "written"), S0_EDITS)
    def test_lint_receives_the_bytes_claude_code_writes_for_a_crlf_edit(
        self, gate, before, old, new, written
    ):
        (gate.root / "pkg" / "crlf.py").write_bytes(before)

        result = gate.run(gate.edit(rel="pkg/crlf.py", old=old, new=new))

        assert result.returncode == 0
        assert bytes.fromhex(gate.stub_calls()[0]["after"]) == written

    def test_a_write_over_a_crlf_file_sends_its_content_verbatim(self, gate):
        (gate.root / "pkg" / "crlf2.py").write_bytes(b"old a\r\nold b\r\n")

        result = gate.run(gate.write("pkg/crlf2.py", "first\nsecond"))

        assert result.returncode == 0
        assert bytes.fromhex(gate.stub_calls()[0]["after"]) == b"first\nsecond"

    def test_a_config_problem_falls_back_to_the_environment_with_a_warning(self, gate):
        (gate.root / ".agentless-mcp.json").write_text("{not json", encoding="utf-8")

        result = gate.run(gate.edit(), AGENTLESS_EROSION_GATE="block")

        assert result.returncode == 0
        record = gate.records()[0]
        assert set(record["modes"].values()) == {"block"}
        assert record["config_warnings"][0].startswith(".agentless-mcp.json is not JSON")

    def test_a_log_that_cannot_be_written_leaves_the_reply_intact(self, gate):
        gate.respond({"stdout": lint_stdout(complexity_finding())})

        result = gate.run(gate.edit(), AGENTLESS_EROSION_LOG=str(gate.tmp_path))

        assert result.returncode == 0
        assert json.loads(result.stdout)["hookSpecificOutput"]["hookEventName"] == "PreToolUse"
        assert b"cannot append" in result.stderr


class TestFailOpen:
    def run_in_process(self, gate, monkeypatch, capsys, payload):
        for name in HOOK_ENV:
            monkeypatch.delenv(name, raising=False)
        for name in ("AGENTLESS_EROSION_BIN", "AGENTLESS_EROSION_LOG", "STUB_CALLS", "STUB_SPEC"):
            monkeypatch.setenv(name, gate.env[name])
        monkeypatch.setattr(tempfile, "tempdir", str(gate.temp_dir))
        stdin = io.TextIOWrapper(io.BytesIO(json.dumps(payload).encode()))
        monkeypatch.setattr(sys, "stdin", stdin)
        code = hook.main()
        return code, capsys.readouterr()

    def test_a_lint_timeout_exits_0_and_logs_degraded(
        self, gate, monkeypatch, capsys, restore_sigterm
    ):
        gate.respond({"sleep": 10})
        monkeypatch.setattr(hook, "DEADLINE_S", 1.5)
        started = time.monotonic()

        code, output = self.run_in_process(gate, monkeypatch, capsys, gate.edit())

        assert time.monotonic() - started < 5
        assert (code, output.out) == (0, "")
        assert gate.records()[0]["reason"].startswith("lint timed out after")
        assert gate.temp_files() == []

    def test_an_internal_error_exits_0_and_logs_degraded(
        self, gate, monkeypatch, capsys, caplog, restore_sigterm
    ):
        def broken(*_args):
            message = "boom"
            raise RuntimeError(message)

        monkeypatch.setattr(hook, "classify", broken)

        code, output = self.run_in_process(gate, monkeypatch, capsys, gate.edit())

        assert (code, output.out) == (0, "")
        assert gate.records()[0]["reason"] == "internal error: RuntimeError: boom"
        assert "erosion gate internal error: tool_use_id=toolu_test1" in caplog.text

    def test_sigterm_removes_the_temp_file_stops_lint_and_exits_0(self, gate):
        gate.respond({"sleep": 30})
        with subprocess.Popen(
            [sys.executable, str(HOOK)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=gate.env,
        ) as process:
            process.stdin.write(json.dumps(gate.edit()).encode())
            process.stdin.close()
            call = self.wait_for_call(gate)
            after = Path(call["argv"][call["argv"].index("--after") + 1])
            assert after.exists()

            process.send_signal(signal.SIGTERM)
            process.wait(timeout=10)

        assert process.returncode == 0
        assert not after.exists()
        assert gate.records()[0]["reason"] == "terminated"
        with pytest.raises(ProcessLookupError):
            os.kill(call["pid"], 0)

    def wait_for_call(self, gate):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            text = gate.calls.read_text(encoding="utf-8") if gate.calls.exists() else ""
            if text.endswith("\n"):
                return json.loads(text.splitlines()[0])
            time.sleep(0.05)
        pytest.fail("the stub lint was not called within 10 s")


def test_the_real_cli_reports_a_branch_added_to_a_complex_function(gate):
    assert CLI.is_file(), f"the console script {CLI} is missing"

    result = gate.run(gate.edit(), AGENTLESS_EROSION_BIN=str(CLI))

    assert result.returncode == 0, result.stderr
    context = json.loads(result.stdout)["hookSpecificOutput"]["additionalContext"]
    assert "pkg/app.py:1: big: complexity 12 -> 13" in context
    [record] = gate.records()
    assert record["outcome"] == "advisory"
    detail = record["findings"][0]["detail"]
    assert (detail["match"], detail["cc_old"], detail["cc_new"]) == ("name", 12, 13)


class TestTheHookAgreesWithThePackage:
    def test_the_suffixes_are_the_ones_the_pair_checks_read(self):
        measured = {
            suffix
            for suffix in TreeSitterExtractor.SUPPORTED_EXTENSIONS
            if erosion.pair_language(f"x{suffix}") is not None
        }

        assert measured == hook.TIER1_SUFFIXES

    def test_every_known_check_is_a_gated_check_and_a_lint_pair_check(self):
        assert set(hook.KNOWN_CHECKS) <= set(projectconfig.GATED_CHECKS)
        assert set(hook.KNOWN_CHECKS) <= set(PAIR_CHECK_NAMES)

    def test_only_clone_delta_is_held_back_from_the_gated_pair_checks(self):
        gated_pairs = set(PAIR_CHECK_NAMES) & set(projectconfig.GATED_CHECKS)

        assert gated_pairs - set(hook.KNOWN_CHECKS) <= {"clone_delta"}
        assert "clone_delta" in projectconfig.GATED_CHECKS
        assert "clone_delta" not in hook.KNOWN_CHECKS

    def test_the_mirrored_constants_match(self):
        assert hook.MODES == projectconfig.GATE_MODES
        assert hook.CONFIG_FILENAME == projectconfig.CONFIG_FILENAME
        assert hook.MAX_CONFIG_BYTES == projectconfig.MAX_CONFIG_BYTES
        assert hook.MAX_OLD_BYTES == DEFAULT_MAX_FILE_BYTES
        assert set(hook.SEVERITIES) == {severity.value for severity in patchlint.Severity}
        assert hook.COVERAGE_CHECK == patchlint.CHECK_COVERAGE

    def test_the_unknown_check_pattern_reads_lints_own_refusal(self):
        refusal = MESSAGES.lint_unknown_check.format(
            name="contract_drift", checks=", ".join(CHECK_NAMES)
        )

        assert hook.offered_checks(2, f"agentless-mcp lint: error: {refusal}") == set(CHECK_NAMES)
        assert hook.offered_checks(1, refusal) is None

    def test_this_branchs_usage_text_has_the_hook_form(self):
        result = subprocess.run([str(CLI), "lint"], capture_output=True, text=True, check=False)

        assert result.returncode == 2
        assert not hook.lacks_hook_form(result.stderr)
        assert hook.lacks_hook_form(RELEASED_USAGE)

    def test_the_settings_example_registers_the_gate_with_a_15_s_timeout(self):
        entries = json.loads(SETTINGS.read_text(encoding="utf-8"))["hooks"]["PreToolUse"]
        [entry] = [entry for entry in entries if entry["matcher"] == "Write|Edit"]
        [command] = entry["hooks"]

        assert command["command"].endswith("agentless_erosion_gate.py")
        assert command["timeout"] == 15
        assert "onFailure" not in command
