"""The erosion gate trial report: finding classes, the seeded sample, labels, and the verdict."""

import csv
import dataclasses
import importlib.util
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from agentless_mcp.core import cache, erosion, grammars, selfrestart

ROOT = Path(__file__).parents[2]
REPORT = ROOT / "contrib" / "hooks" / "erosion_gate_report.py"
HOOK = ROOT / "contrib" / "hooks" / "agentless_erosion_gate.py"
GIT_DIR = str(Path(shutil.which("git") or "git").parent)
KEY = ("get-default-empty", "parse", 'data.get("items", [])')
COPY = "4" * 64
OTHER = "7" * 64
CLI = Path(sys.executable).parent / "agentless-mcp"

STUB = """#!{python}
import json
import os
import sys
from pathlib import Path

calls = Path(os.environ["STUB_CALLS"])
with calls.open("a") as handle:
    handle.write("call\\n")
responses = json.loads(Path(os.environ["STUB_SPEC"]).read_text())
sys.stdout.write(responses[len(calls.read_text().splitlines()) - 1])
"""


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    # A dataclass resolves its string annotations through sys.modules while the class is built.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


report = load(REPORT, "erosion_gate_report")
hook = load(HOOK, "agentless_erosion_gate")


def complexity(*, unit="big", cc_old=12, cc_new=13, threshold=10, match="name"):
    return {
        "check": "complexity_delta",
        "severity": "warning",
        "mode": "advisory",
        "path": "pkg/app.py",
        "line": 1,
        "message": f"{unit}: complexity {cc_old} -> {cc_new}",
        "evidence": f"cc {cc_old} -> {cc_new}",
        "detail": {
            "unit": unit,
            "match": match,
            "cc_old": cc_old,
            "cc_new": cc_new,
            "sloc_old": None if cc_old is None else 20,
            "sloc_new": 22,
            "threshold": threshold,
        },
    }


def contract(key=KEY):
    return {
        "check": "contract_drift",
        "severity": "warning",
        "mode": "advisory",
        "path": "pkg/app.py",
        "line": 9,
        "message": "parse: a silent default",
        "evidence": key[2],
        "detail": dict(zip(("rule", "unit", "capture"), key, strict=True)),
    }


def clone(key=COPY, *, sites=None):
    listed = [{"path": "pkg/app.py", "line": 1, "end_line": 6}] if sites is None else sites
    return {
        "check": "clone_delta",
        "severity": "advisory",
        "mode": "advisory",
        "path": "pkg/app.py",
        "line": 20,
        "message": "lines 20-25 add a copy of a block",
        "evidence": "also at pkg/app.py:1",
        "detail": {
            "key": key,
            "line": 20,
            "end_line": 25,
            "sites": listed,
            "sites_total": len(listed),
        },
    }


def copies(*keys, path="pkg/app.py"):
    rows = [
        {"path": path, "key": key, "line": 10 * index + 1, "end_line": 10 * index + 6}
        for index, key in enumerate(keys)
    ]
    return {"clone_instances": rows}


def observed(units=None, keys=None, *, path="pkg/app.py"):
    document = {}
    if units is not None:
        document["units_over_threshold"] = [
            {"path": path, "unit": unit, "line": 1, "cc": cc, "sloc": 20} for unit, cc in units
        ]
        document["units_changed"] = []
    if keys is not None:
        document["contract_keys"] = [
            {"path": path, "rule": rule, "unit": unit, "capture": capture}
            for rule, unit, capture in keys
        ]
    return document or None


class Trial:
    def __init__(self, tmp_path):
        self.tmp_path = tmp_path
        self.log = tmp_path / "gate.jsonl"
        self.lines = []
        self.results = {}

    def add(self, tool_use_id, *, session="s1", path="/repo/pkg/app.py", transcript="t1", **fields):
        record = {
            "v": 1,
            "ts": 1791600000.0 + len(self.lines),
            "latency_ms": 200.0,
            "session_id": session,
            "transcript_path": str(self.tmp_path / f"{transcript}.jsonl"),
            "tool_use_id": tool_use_id,
            "tool_name": "Edit",
            "path": path,
            "binary": "/opt/agentless-mcp",
            "outcome": "advisory",
            "reason": None,
            "findings": [],
            "observed": None,
        }
        record.update(fields)
        self.lines.append(json.dumps(record))

    def result(self, tool_use_id, *, rejected=False, transcript="t1"):
        self.results.setdefault(transcript, []).append((tool_use_id, rejected))

    def write(self):
        self.log.write_text("\n".join(self.lines) + "\n", encoding="utf-8")
        for transcript, results in self.results.items():
            lines = [json.dumps({"type": "summary", "note": "not a tool result"})]
            for tool_use_id, rejected in results:
                item = {"type": "tool_result", "tool_use_id": tool_use_id, "content": "done"}
                if rejected:
                    item["is_error"] = True
                lines.append(json.dumps({"type": "user", "message": {"content": [item]}}))
            path = self.tmp_path / f"{transcript}.jsonl"
            path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return self.log

    def classified(self):
        records = report.read_log(self.write())
        return report.classify(records, report.read_transcripts(records))


def classes(classified):
    return {item.record.tool_use_id: item.cls for item in classified}


def run_report(*args):
    return report.main([str(arg) for arg in args])


@pytest.fixture
def trial(tmp_path):
    return Trial(tmp_path)


class TestClasses:
    def test_each_finding_lands_in_its_class(self, trial):
        trial.add("rejected", findings=[complexity()])
        trial.result("rejected", rejected=True)
        trial.add("no-transcript", transcript="absent", findings=[complexity()])
        trial.add("no-result", findings=[complexity()])
        trial.add("unobserved", session="s4", findings=[complexity()])
        trial.result("unobserved")
        trial.add("grew-then-shrank", session="s5", findings=[complexity()])
        trial.result("grew-then-shrank")
        trial.add("later-s5", session="s5", outcome="clean", observed=observed([("big", 12)]))
        trial.result("later-s5")
        trial.add("crossed", session="s6", findings=[complexity(cc_old=9, cc_new=11)])
        trial.result("crossed")
        trial.add("later-s6", session="s6", outcome="clean", observed=observed([]))
        trial.result("later-s6")
        trial.add("still-grown", session="s7", findings=[complexity()])
        trial.result("still-grown")
        trial.add("later-s7", session="s7", outcome="clean", observed=observed([("big", 13)]))
        trial.result("later-s7")

        assert classes(trial.classified()) == {
            "rejected": "not applied",
            "no-transcript": "unresolved",
            "no-result": "unresolved",
            "unobserved": "unobserved",
            "grew-then-shrank": "acted on",
            "crossed": "acted on",
            "still-grown": "not acted on",
        }

    def test_a_later_record_without_observed_state_leaves_the_finding_unobserved(self, trial):
        trial.add("finding", findings=[complexity()])
        trial.result("finding")
        trial.add("degraded-later", outcome="degraded", reason="lint timed out after 9.5 s")
        trial.result("degraded-later")

        assert classes(trial.classified()) == {"finding": "unobserved"}

    def test_a_later_call_that_was_rejected_is_not_evidence(self, trial):
        trial.add("finding", findings=[complexity()])
        trial.result("finding")
        trial.add("rejected-later", outcome="clean", observed=observed([]))
        trial.result("rejected-later", rejected=True)

        assert classes(trial.classified()) == {"finding": "unobserved"}

    def test_another_session_or_path_is_not_evidence(self, trial):
        trial.add("finding", findings=[complexity()])
        trial.result("finding")
        trial.add("other-session", session="s2", outcome="clean", observed=observed([]))
        trial.result("other-session")
        trial.add("other-path", path="/repo/b.py", outcome="clean", observed=observed([]))
        trial.result("other-path")

        assert classes(trial.classified()) == {"finding": "unobserved"}

    @pytest.mark.parametrize(
        ("at_finding", "later", "expected"),
        [
            ([KEY, KEY], [KEY], "acted on"),
            ([KEY, KEY], [KEY, KEY], "not acted on"),
            (None, [], "acted on"),
            (None, [KEY], "not acted on"),
        ],
    )
    def test_a_contract_key_counts_as_a_multiset(self, trial, at_finding, later, expected):
        at = None if at_finding is None else observed(keys=at_finding)
        trial.add("finding", findings=[contract()], observed=at)
        trial.result("finding")
        trial.add("later", outcome="clean", observed=observed(keys=later))
        trial.result("later")

        assert classes(trial.classified()) == {"finding": expected}

    def test_a_renamed_function_keeps_its_contract_hit(self, trial):
        renamed = ("get-default-empty", "parse_response", 'data.get("items", [])')
        trial.add("finding", findings=[contract()], observed=observed(keys=[KEY]))
        trial.result("finding")
        trial.add("later", outcome="clean", observed=observed(keys=[renamed]))
        trial.result("later")

        assert classes(trial.classified()) == {"finding": "not acted on"}

    def test_a_row_for_another_path_is_not_this_findings_row(self, trial):
        trial.add("unit", session="s1", findings=[complexity()])
        trial.result("unit")
        trial.add(
            "unit-later",
            session="s1",
            outcome="clean",
            observed=observed([("big", 13)], path="pkg/other.py"),
        )
        trial.result("unit-later")
        trial.add("hit", session="s2", findings=[contract()], observed=observed(keys=[KEY]))
        trial.result("hit")
        trial.add(
            "hit-later",
            session="s2",
            outcome="clean",
            observed=observed(keys=[KEY], path="pkg/other.py"),
        )
        trial.result("hit-later")

        assert classes(trial.classified()) == {"unit": "acted on", "hit": "acted on"}


class TestCloneClasses:
    @pytest.mark.parametrize(
        ("at_finding", "later", "expected"),
        [
            ([COPY, COPY], [COPY], "acted on"),
            ([COPY, COPY], [], "acted on"),
            ([COPY, COPY], [COPY, COPY], "not acted on"),
            ([COPY, COPY], [COPY, COPY, OTHER], "not acted on"),
            (None, [], "acted on"),
            (None, [COPY], "not acted on"),
        ],
    )
    def test_a_copy_counts_by_its_key_in_its_file(self, trial, at_finding, later, expected):
        at = None if at_finding is None else copies(*at_finding)
        trial.add("finding", findings=[clone()], observed=at)
        trial.result("finding")
        trial.add("later", outcome="clean", observed=copies(*later))
        trial.result("later")

        assert classes(trial.classified()) == {"finding": expected}

    def test_a_later_record_without_clone_instances_leaves_it_unobserved(self, trial):
        trial.add("finding", findings=[clone()], observed=copies(COPY, COPY))
        trial.result("finding")
        trial.add("later", outcome="clean", observed=observed([("big", 9)]))
        trial.result("later")

        assert classes(trial.classified()) == {"finding": "unobserved"}

    def test_a_row_in_another_file_is_not_this_copy(self, trial):
        trial.add("finding", findings=[clone()], observed=copies(COPY, COPY))
        trial.result("finding")
        trial.add("later", outcome="clean", observed=copies(COPY, COPY, path="pkg/other.py"))
        trial.result("later")

        assert classes(trial.classified()) == {"finding": "acted on"}


class TestFigures:
    def test_the_report_prints_the_classes_and_the_effective_rate(self, trial, capsys):
        for session, cc in (("a", 12), ("b", 13), ("c", 12)):
            trial.add(f"finding-{session}", session=session, findings=[complexity()])
            trial.result(f"finding-{session}")
            trial.add(
                f"later-{session}",
                session=session,
                outcome="clean",
                observed=observed([("big", cc)]),
            )
            trial.result(f"later-{session}")

        assert run_report(trial.write()) == 0

        out = capsys.readouterr().out
        assert "complexity_delta: 3 findings" in out
        assert "not applied 0, unresolved 0, unobserved 0, acted on 2, not acted on 1" in out
        assert "effective false-positive rate: 0.33 (1 of 3 observed)" in out
        assert "match pass: body 0, name 3, collision 0, new 0" in out
        assert "Outcomes: clean 3, advisory 3, blocked 0, degraded 0, skipped 0." in out
        assert report.CONDITION_3 in out

    def test_no_observed_finding_gives_no_rate(self, trial, capsys):
        trial.add("finding", findings=[complexity()])
        trial.result("finding")

        run_report(trial.write())

        assert "effective false-positive rate: n/a (0 of 0 observed)" in capsys.readouterr().out

    def test_a_partial_transcript_line_is_counted_not_hidden(self, trial, capsys):
        trial.add("finding", findings=[complexity()])
        trial.result("finding")
        trial.write()
        transcript = trial.tmp_path / "t1.jsonl"
        transcript.write_text(transcript.read_text() + '{"type": "user", "mess', encoding="utf-8")

        run_report(trial.log)

        assert "has 1 unreadable lines" in capsys.readouterr().out


class TestSample:
    def build(self, trial, complexity_findings=40, contract_findings=5):
        for index in range(complexity_findings):
            trial.add(f"c{index}", findings=[complexity()])
            trial.result(f"c{index}")
        for index in range(contract_findings):
            trial.add(f"k{index}", findings=[contract()])
            trial.result(f"k{index}")
        trial.add("rejected", findings=[complexity()])
        trial.result("rejected", rejected=True)
        return trial.write()

    def ids(self, path):
        with path.open(newline="", encoding="utf-8") as handle:
            return [row["finding_id"] for row in csv.DictReader(handle)]

    def test_the_same_seed_draws_the_same_applied_findings(self, trial, tmp_path):
        log = self.build(trial)
        first, second, other = tmp_path / "a.csv", tmp_path / "b.csv", tmp_path / "c.csv"

        for out, seed in ((first, 0), (second, 0), (other, 7)):
            assert run_report(log, "--sample-out", out, "--seed", seed) == 0

        assert self.ids(first) == self.ids(second)
        assert self.ids(first) != self.ids(other)
        assert sum(row.startswith("c") for row in self.ids(first)) == 30
        assert sum(row.startswith("k") for row in self.ids(first)) == 5
        assert "rejected#0" not in self.ids(first)

    def test_the_draw_does_not_depend_on_the_hash_seed(self, trial, tmp_path):
        log = self.build(trial)
        drawn = []
        for hash_seed in ("1", "2"):
            out = tmp_path / f"hash-{hash_seed}.csv"
            subprocess.run(
                [sys.executable, str(REPORT), str(log), "--sample-out", str(out)],
                env={"PATH": GIT_DIR, "PYTHONHASHSEED": hash_seed},
                capture_output=True,
                check=True,
                timeout=60,
            )
            drawn.append(self.ids(out))

        assert drawn[0] == drawn[1]

    def test_an_existing_sample_file_is_never_overwritten(self, trial, tmp_path):
        log = self.build(trial)
        out = tmp_path / "labels.csv"
        out.write_text("filled\n", encoding="utf-8")

        assert run_report(log, "--sample-out", out) == 2
        assert out.read_text(encoding="utf-8") == "filled\n"


class TestLabels:
    def labeled(self, trial, tmp_path, labels):
        for index in range(50):
            trial.add(f"c{index}", findings=[complexity()])
            trial.result(f"c{index}")
        log = trial.write()
        out = tmp_path / "sample.csv"
        assert run_report(log, "--sample-out", out) == 0
        with out.open(newline="", encoding="utf-8") as handle:
            rows = list(csv.DictReader(handle))
        for row, label in zip(rows, labels, strict=False):
            row["label"] = label
        with out.open("w", newline="", encoding="utf-8") as handle:
            writer = csv.DictWriter(handle, fieldnames=report.CSV_FIELDS)
            writer.writeheader()
            writer.writerows(rows)
        return log, out

    def test_27_of_30_correct_qualifies_on_conditions_1_and_2(self, trial, tmp_path, capsys):
        log, labels = self.labeled(trial, tmp_path, ["correct"] * 27 + ["wrong"] * 3)
        capsys.readouterr()

        assert run_report(log, "--labels", labels) == 0

        out = capsys.readouterr().out
        assert "labeled precision: 0.90 (27 of 30 correct), 30 of 30 labeled" in out
        assert "flip: qualifies on conditions 1 and 2" in out

    def test_an_unlabeled_row_keeps_the_check_from_qualifying(self, trial, tmp_path, capsys):
        log, labels = self.labeled(trial, tmp_path, ["correct"] * 29)
        capsys.readouterr()

        run_report(log, "--labels", labels)

        assert "does not qualify: 29 of 30 sampled findings are labeled." in capsys.readouterr().out

    @pytest.mark.parametrize(
        ("change", "reason"),
        [
            ({"label": "maybe"}, "label 'maybe' is not correct, wrong, or empty"),
            ({"finding_id": "toolu_unknown#0"}, "toolu_unknown#0 is not an applied finding"),
            ({"check": "contract_drift"}, "is a complexity_delta finding, not contract_drift"),
        ],
    )
    def test_a_row_the_log_cannot_match_stops_the_report(
        self, trial, tmp_path, capsys, change, reason
    ):
        log, labels = self.labeled(trial, tmp_path, ["correct"] * 30)
        with labels.open(newline="", encoding="utf-8") as handle:
            rows = list(csv.DictReader(handle))
        rows[3].update(change)
        with labels.open("w", newline="", encoding="utf-8") as handle:
            writer = csv.DictWriter(handle, fieldnames=report.CSV_FIELDS)
            writer.writeheader()
            writer.writerows(rows)

        assert run_report(log, "--labels", labels) == 1

        error = capsys.readouterr().err
        assert f"{labels}:5:" in error
        assert reason in error

    def test_a_duplicate_row_stops_the_report(self, trial, tmp_path, capsys):
        log, labels = self.labeled(trial, tmp_path, ["correct"] * 30)
        lines = labels.read_text(encoding="utf-8").splitlines()
        labels.write_text("\n".join([*lines, lines[1]]) + "\n", encoding="utf-8")

        assert run_report(log, "--labels", labels) == 1
        assert "appears twice" in capsys.readouterr().err


class TestVerdict:
    @pytest.mark.parametrize(
        ("applied", "labels", "start"),
        [
            (50, report.LabelCount(27, 3, 0), "qualifies on conditions 1 and 2"),
            (49, report.LabelCount(30, 0, 0), "does not qualify: 49 applied findings, under 50"),
            (50, report.LabelCount(26, 4, 0), "does not qualify: precision 0.87, under 0.90"),
            (50, report.LabelCount(0, 0, 0), "does not qualify: 0 of 0 sampled findings"),
            (50, None, "condition 1 holds"),
        ],
    )
    def test_the_flip_criterion(self, applied, labels, start):
        assert report.verdict(applied, labels).startswith(start)


class TestMalformedInput:
    @pytest.mark.parametrize(
        ("line", "reason"),
        [
            ("not json", "not JSON"),
            ('{"v": 2}', "log version 2, not 1"),
            ('{"v": 1, "outcome": "advisory"}', "observed is missing"),
            ('{"v": 1, "outcome": "lost"}', "outcome 'lost' is not one of"),
        ],
    )
    def test_a_malformed_log_line_stops_with_its_line_number(self, trial, capsys, line, reason):
        trial.add("fine", outcome="skipped", reason="every check is off")
        trial.lines.append(line)

        assert run_report(trial.write()) == 1

        error = capsys.readouterr().err
        assert f"{trial.log}:2:" in error
        assert reason in error

    @pytest.mark.parametrize(
        ("rows", "reason"),
        [
            ({"units_over_threshold": None}, "units_over_threshold is not list"),
            (
                {"units_over_threshold": [{"unit": "big", "line": 1, "cc": 13, "sloc": 2}]},
                "units_over_threshold[0]: path is missing",
            ),
            ({"contract_keys": [list(KEY)]}, "contract_keys[0] is not an object"),
            (
                {
                    "units_changed": [
                        {
                            "path": "a.py",
                            "unit": "f",
                            "line": 1,
                            "match": "name",
                            "cc_old": 1,
                            "cc_new": "2",
                            "sloc_old": 1,
                            "sloc_new": 2,
                        }
                    ]
                },
                "cc_new is not int",
            ),
        ],
    )
    def test_a_malformed_observed_row_stops_the_report(self, trial, capsys, rows, reason):
        trial.add("finding", outcome="clean", observed=rows)

        assert run_report(trial.write()) == 1
        assert reason in capsys.readouterr().err

    @pytest.mark.parametrize(
        ("change", "reason"),
        [
            (lambda detail: detail.pop("key"), "detail: key is missing"),
            (lambda detail: detail.update(sites={}), "detail: sites is not list"),
            (lambda detail: detail.update(sites=[[1, 2]]), "detail.sites[0] is not an object"),
            (
                lambda detail: detail.update(sites=[{"path": "a.py", "line": "1", "end_line": 2}]),
                "detail.sites[0]: line is not int",
            ),
            (lambda detail: detail.update(sites_total=None), "detail: sites_total is not int"),
        ],
    )
    def test_a_malformed_clone_detail_stops_the_report(self, trial, capsys, change, reason):
        finding = clone()
        change(finding["detail"])
        trial.add("finding", findings=[finding])

        assert run_report(trial.write()) == 1
        assert reason in capsys.readouterr().err

    def test_a_malformed_clone_instance_stops_the_report(self, trial, capsys):
        trial.add("finding", outcome="clean", observed={"clone_instances": [{"path": "a.py"}]})

        assert run_report(trial.write()) == 1
        assert "clone_instances[0]: key is missing" in capsys.readouterr().err

    def test_a_complexity_finding_without_its_numbers_stops_the_report(self, trial, capsys):
        finding = complexity()
        del finding["detail"]["threshold"]
        trial.add("finding", findings=[finding])

        assert run_report(trial.write()) == 1
        assert "threshold is missing" in capsys.readouterr().err


class TestTheReportAgreesWithTheHookAndThePackage:
    def test_the_match_passes_and_detail_keys_match_the_package(self):
        delta = erosion.UnitDelta("u", "name", 1, 2, 3, 4, 10).as_dict()

        assert report.MATCH_PASSES == (
            erosion.MATCH_BODY,
            erosion.MATCH_NAME,
            erosion.MATCH_COLLISION,
            erosion.MATCH_NEW,
        )
        assert set(complexity()["detail"]) == set(delta)

    def test_the_clone_detail_and_rows_match_the_package(self):
        site = erosion.CloneSite("a.py", 1, 6)
        detail = erosion.CloneCopy(COPY, 20, 25, (site,), 1).as_dict()
        row = erosion.CloneState("a.py", COPY, 1, 6).as_dict()

        assert set(clone()["detail"]) == set(detail)
        assert set(clone()["detail"]["sites"][0]) == set(detail["sites"][0])
        assert set(copies(COPY)["clone_instances"][0]) == set(row)

    def test_the_outcomes_version_and_checks_match_the_hook(self):
        assert set(report.OUTCOMES) == {
            hook.CLEAN,
            hook.ADVISED,
            hook.BLOCKED,
            hook.DEGRADED,
            hook.SKIPPED,
        }
        assert report.LOG_VERSION == hook.LOG_VERSION
        assert report.KNOWN_CHECKS == hook.KNOWN_CHECKS

    def test_the_report_reads_the_log_the_hook_writes(self, tmp_path, make_git_repo):
        source = "def big(x):\n" + "".join(
            f"    if x == {i}:\n        return {i}\n" for i in range(11)
        )
        root = make_git_repo({"pkg/app.py": source + "    return -1\n"})
        stub = tmp_path / "agentless-mcp"
        stub.write_text(STUB.format(python=sys.executable), encoding="utf-8")
        stub.chmod(0o755)
        lint = {"id": "pkg/app.py"}
        first = {**lint, "findings": [complexity()], "observed": observed([("big", 13)])}
        second = {**lint, "findings": [], "observed": observed([("big", 12)])}
        (tmp_path / "spec.json").write_text(
            json.dumps([json.dumps({"candidates": [first]}), json.dumps({"candidates": [second]})]),
            encoding="utf-8",
        )
        env = {
            "PATH": GIT_DIR,
            "HOME": str(tmp_path),
            "TMPDIR": str(tmp_path),
            "GIT_CONFIG_GLOBAL": os.devnull,
            "AGENTLESS_EROSION_BIN": str(stub),
            "AGENTLESS_EROSION_LOG": str(tmp_path / "gate.jsonl"),
            "STUB_CALLS": str(tmp_path / "calls.txt"),
            "STUB_SPEC": str(tmp_path / "spec.json"),
        }
        transcript = tmp_path / "t1.jsonl"
        for tool_use_id, old, new in (
            ("toolu_a", "    return -1\n", "    if x < 0:\n        return 0\n    return -1\n"),
            ("toolu_b", "    if x == 0:\n", "    if x == 100:\n"),
        ):
            payload = {
                "session_id": "s1",
                "transcript_path": str(transcript),
                "tool_use_id": tool_use_id,
                "tool_name": "Edit",
                "cwd": str(root),
                "tool_input": {
                    "file_path": str(root / "pkg" / "app.py"),
                    "old_string": old,
                    "new_string": new,
                    "replace_all": False,
                },
            }
            subprocess.run(
                [sys.executable, str(HOOK)],
                input=json.dumps(payload).encode(),
                env=env,
                capture_output=True,
                check=True,
                timeout=60,
            )
        results = [
            {"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": i}]}}
            for i in ("toolu_a", "toolu_b")
        ]
        transcript.write_text("\n".join(json.dumps(r) for r in results) + "\n", encoding="utf-8")

        records = report.read_log(tmp_path / "gate.jsonl")
        classified = report.classify(records, report.read_transcripts(records))

        assert [record.outcome for record in records] == ["advisory", "clean"]
        assert [(item.finding_id, item.cls) for item in classified] == [("toolu_a#0", "acted on")]


SUMMED = (
    "def total(items, scale=1, offset=None):\n"
    "    out = 0 * scale + len(items) - (offset or 0)\n"
    "    for item in sorted(items, key=abs, reverse=True):\n"
    "        out += item * scale - 0 + max(item, 0)\n"
    "    return round(out, 2) if out else 0\n"
)
PASTED = "\n\n" + SUMMED.replace("def total(", "def again(")
PINNED_ENV = (
    grammars.ENV_CACHE_DIR,
    grammars.ENV_NO_AUTO_WARM,
    cache.ENV_NO_AUTO_INDEX,
    selfrestart.ENV_NO_AUTO_RESTART,
    cache.ENV_MAX_CACHE_BYTES,
)


def hook_env(tmp_path):
    """The environment the real hook runs in, with its log and cache under tmp_path."""
    env = {name: os.environ[name] for name in PINNED_ENV if name in os.environ}
    env.update(
        PATH=GIT_DIR,
        HOME=str(tmp_path),
        TMPDIR=str(tmp_path),
        GIT_CONFIG_GLOBAL=os.devnull,
        XDG_CACHE_HOME=str(tmp_path / "xdg"),
        AGENTLESS_EROSION_BIN=str(CLI),
        AGENTLESS_EROSION_LOG=str(tmp_path / "gate.jsonl"),
    )
    return env


@dataclasses.dataclass(frozen=True)
class HookSession:
    """One hook session's environment, transcript, repository, and edited file."""

    env: dict
    transcript: Path
    root: Path
    target: Path

    def edit(self, tool_use_id, old, new):
        """Send one Edit payload for the target through the real hook."""
        payload = {
            "session_id": "s1",
            "transcript_path": str(self.transcript),
            "tool_use_id": tool_use_id,
            "tool_name": "Edit",
            "cwd": str(self.root),
            "tool_input": {
                "file_path": str(self.target),
                "old_string": old,
                "new_string": new,
                "replace_all": False,
            },
        }
        subprocess.run(
            [sys.executable, str(HOOK)],
            input=json.dumps(payload).encode(),
            env=self.env,
            capture_output=True,
            check=True,
            timeout=60,
        )


def write_tool_results(transcript, tool_use_ids):
    """Write a transcript in which each tool call returned a result."""
    results = [
        {"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": i}]}}
        for i in tool_use_ids
    ]
    transcript.write_text("\n".join(json.dumps(r) for r in results) + "\n", encoding="utf-8")


def test_a_removed_copy_is_acted_on_through_the_real_hook_and_cli(tmp_path, make_git_repo):
    root = make_git_repo({"pkg/app.py": SUMMED})
    target = root / "pkg" / "app.py"
    transcript = tmp_path / "t1.jsonl"
    session = HookSession(hook_env(tmp_path), transcript, root, target)
    # The hook form reads clone blocks from the tag cache only.
    subprocess.run(
        [str(CLI), "index", "--repo", str(root)],
        env=session.env,
        capture_output=True,
        check=True,
        timeout=60,
    )

    last = "    return round(out, 2) if out else 0\n"
    session.edit("toolu_a", last, last + PASTED)
    target.write_text(SUMMED + PASTED, encoding="utf-8")
    session.edit("toolu_b", PASTED, "")
    write_tool_results(transcript, ("toolu_a", "toolu_b"))

    records = report.read_log(tmp_path / "gate.jsonl")
    classified = report.classify(records, report.read_transcripts(records))

    assert [record.outcome for record in records] == ["advisory", "clean"]
    (finding,) = records[0].findings
    assert finding.check == "clone_delta"
    assert len(records[0].observed.clone_instances) == 2
    assert records[1].observed.clone_instances == ()
    assert [(item.finding_id, item.cls) for item in classified] == [("toolu_a#0", "acted on")]
