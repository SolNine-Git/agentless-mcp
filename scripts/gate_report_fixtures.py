"""Write the gate-report fixtures: each input of the erosion gate report with the reference answer.

Development-time only. Run it from the repository root with the project environment:

    uv run python scripts/gate_report_fixtures.py

The report cases run the reference report in process, each in its own temporary directory, which
the fixtures write as <SANDBOX>. The hook log case runs the reference hook as a process with a stub
lint, as the reference tests do. A case whose input the Rust build reads by a ledger deviation
records the reference's own outcome beside the outcome of the adjusted input that the Rust build
must equal. With --check, the script compares its output with the committed fixtures and writes
nothing.
"""

import argparse
import contextlib
import csv
import functools
import importlib.util
import io
import json
import math
import os
import random
import re
import shutil
import struct
import subprocess
import sys
import tempfile
from collections.abc import Callable, Iterator
from pathlib import Path
from types import ModuleType
from typing import Any, cast

from fixture_files import Fixture, Identity, render, run_git, write_or_check_files

ROOT = Path(__file__).resolve().parents[1]
HOOK_FIXTURES = ROOT / "crates" / "hook" / "fixtures" / "report"
CLI_FIXTURES = ROOT / "crates" / "cli" / "fixtures" / "gate_report"
REPORT_PATH = ROOT / "contrib" / "hooks" / "erosion_gate_report.py"
HOOK_PATH = ROOT / "contrib" / "hooks" / "agentless_erosion_gate.py"
PLACEHOLDER = "<SANDBOX>"
RUST_PROG = "agentless-mcp gate-report"
# Without a terminal the reference parser wraps at 80 columns, as the Rust parser does.
NO_TERMINAL_COLUMNS = "80"
IDENTITY = Identity("report tests", "report@example.invalid", "2026-02-01T00:00:00Z")
KEY = ("get-default-empty", "parse", 'data.get("items", [])')
COPY = "4" * 64
OTHER = "7" * 64
CHUNK = 8192
FIELD_LIMIT = 131072
SAMPLE_SIZE = 30
SMALL_SAMPLE = 5
HOOK_SECONDS = 60
SAFE_PATH = re.compile(r"[A-Za-z0-9/._-]+")
DELETE = object()
BOM = chr(0xFEFF)
LINE_SEPARATOR = chr(0x2028)
EM_SPACE = chr(0x2003)
EURO = chr(0x20AC)
# The exceptions that end the reference in a traceback on the inputs here.
CRASHES = (ArithmeticError, LookupError, OSError, RecursionError, TypeError, ValueError)
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

Json = dict[str, Any]
Rows = list[dict[str, str]]


def load(path: Path, name: str) -> ModuleType:
    """Load a hook script as a module, registered so that its dataclasses resolve."""
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        message = f"refused: {path} does not load as a module"
        raise SystemExit(message)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


report = load(REPORT_PATH, "erosion_gate_report")


def latin(data: bytes) -> str:
    """Return bytes as the latin-1 text that the fixtures store them as."""
    return data.decode("latin-1")


def complexity(*, cc_old: int | None = 12, cc_new: int = 13, **extra: object) -> Json:
    """Return one complexity_delta finding as the hook logs it."""
    return {
        "check": "complexity_delta",
        "severity": "warning",
        "mode": "advisory",
        "path": "pkg/app.py",
        "line": 1,
        "message": f"big: complexity {cc_old} -> {cc_new}",
        "evidence": f"cc {cc_old} -> {cc_new}",
        "detail": {
            "unit": "big",
            "match": "name",
            "cc_old": cc_old,
            "cc_new": cc_new,
            "sloc_old": None if cc_old is None else 20,
            "sloc_new": 22,
            "threshold": 10,
        },
        **extra,
    }


def contract() -> Json:
    """Return one contract_drift finding as the hook logs it."""
    return {
        "check": "contract_drift",
        "severity": "warning",
        "mode": "advisory",
        "path": "pkg/app.py",
        "line": 9,
        "message": "parse: a silent default",
        "evidence": KEY[2],
        "detail": dict(zip(("rule", "unit", "capture"), KEY, strict=True)),
    }


def clone() -> Json:
    """Return one clone_delta finding as the hook logs it."""
    listed = [{"path": "pkg/app.py", "line": 1, "end_line": 6}]
    return {
        "check": "clone_delta",
        "severity": "advisory",
        "mode": "advisory",
        "path": "pkg/app.py",
        "line": 20,
        "message": "lines 20-25 add a copy of a block",
        "evidence": "also at pkg/app.py:1",
        "detail": {"key": COPY, "line": 20, "end_line": 25, "sites": listed, "sites_total": 1},
    }


def copies(*keys: str, path: str = "pkg/app.py") -> Json:
    """Return an observed state that holds one clone instance per key."""
    rows = [
        {"path": path, "key": key, "line": 10 * index + 1, "end_line": 10 * index + 6}
        for index, key in enumerate(keys)
    ]
    return {"clone_instances": rows}


def observed(
    units: list[tuple[str, int]] | None = None,
    keys: list[tuple[str, str, str]] | None = None,
    *,
    path: str = "pkg/app.py",
) -> Json | None:
    """Return an observed state with units over the threshold and contract keys."""
    document: Json = {}
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


def captured(call: Callable[[], object]) -> tuple[str, str, object, str | None]:
    """Run ``call`` with stdout and stderr captured; return them, its value, and a crash line."""
    out, err = io.StringIO(), io.StringIO()
    crash = None
    value: object = None
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        try:
            value = call()
        except SystemExit as stop:
            value = stop.code
        except CRASHES as error:
            crash = f"{type(error).__name__}: {error}"
            value = 1
    return out.getvalue(), err.getvalue(), value, crash


class Case:
    """One report case: its steps in a sandbox, then the files that the runs made."""

    def __init__(self, sandbox: Path) -> None:
        self.sandbox = sandbox
        self.steps: list[list[object]] = []
        self.put_files: dict[str, bytes] = {}
        self.log: Path | None = None
        self.deviation: str | None = None
        self.reference: list[object] = []

    def shown(self, text: str) -> str:
        """Return text with the sandbox written as the placeholder."""
        return text.replace(str(self.sandbox), PLACEHOLDER)

    def put(self, rel: str, data: bytes | str) -> None:
        """Write a file and record it."""
        raw = data.encode("utf-8") if isinstance(data, str) else data
        target = self.sandbox / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(raw)
        self.put_files[rel] = raw
        shown = raw.replace(str(self.sandbox).encode(), PLACEHOLDER.encode())
        self.steps.append(["put", rel, latin(shown)])

    def mkdir(self, rel: str) -> None:
        """Make a directory and record it."""
        (self.sandbox / rel).mkdir(parents=True, exist_ok=True)
        self.steps.append(["mkdir", rel])

    def path(self, rel: str) -> str:
        """Return the absolute text of a sandbox path."""
        return str(self.sandbox / rel)

    def run(self, *argv: str) -> None:
        """Run the reference report on argv in process and record the run."""
        args = report._parser().parse_args(list(argv))
        out, err, code, crash = captured(lambda: report.main(list(argv)))
        request = {
            "log": self.shown(args.log),
            "sample_out": None if args.sample_out is None else self.shown(args.sample_out),
            "labels": None if args.labels is None else self.shown(args.labels),
            "seed": str(args.seed),
        }
        self.steps.append(["run", request, self.shown(out), self.shown(err), code, crash])

    def made_files(self) -> list[list[str]]:
        """Return each file that a run made or changed, with its bytes."""
        made = []
        for path in sorted(self.sandbox.rglob("*")):
            rel = str(path.relative_to(self.sandbox))
            if path.is_file() and self.put_files.get(rel) != path.read_bytes():
                shown = path.read_bytes().replace(str(self.sandbox).encode(), b"<SANDBOX>")
                made.append([rel, latin(shown)])
        return made

    def classified(self) -> list[list[str]] | None:
        """Return the reference's class of each finding in the case's log, or null."""
        if self.log is None:
            return None
        try:
            records = report.read_log(self.log)
            found = report.classify(records, report.read_transcripts(records))
        except (report.ReportError, *CRASHES):
            return None
        return [[str(item.finding_id), str(item.cls)] for item in found]

    def fixture(self, name: str) -> Json:
        """Return the case as one fixture row."""
        return {
            "name": name,
            "steps": self.steps,
            "files": self.made_files(),
            "classified": self.classified(),
            "deviation": self.deviation,
            "reference": self.reference,
        }


@contextlib.contextmanager
def sandbox() -> Iterator[Path]:
    """Yield a fresh resolved directory whose path the fixtures can write as the placeholder."""
    with tempfile.TemporaryDirectory(prefix="gate-report-") as made:
        path = Path(made).resolve()
        if not SAFE_PATH.fullmatch(str(path)):
            message = f"refused: {path} holds a character that a repr escapes"
            raise SystemExit(message)
        yield path


class Trial:
    """A gate log and its transcripts, built record by record as the reference tests build them."""

    def __init__(self, case: Case) -> None:
        self.case = case
        self.lines: list[str] = []
        self.results: list[tuple[str, object]] = []

    def add(self, tool_use_id: str, **fields: Any) -> None:
        """Add one record with the reference tests' defaults; session, path and transcript name."""
        transcript = fields.pop("transcript", "t1")
        record: Json = {
            "v": 1,
            "ts": 1791600000.0 + len(self.lines),
            "latency_ms": 200.0,
            "session_id": fields.pop("session", "s1"),
            "transcript_path": self.case.path(f"{transcript}.jsonl"),
            "tool_use_id": tool_use_id,
            "tool_name": "Edit",
            "path": fields.pop("path", "/repo/pkg/app.py"),
            "binary": "/opt/agentless-mcp",
            "outcome": "advisory",
            "reason": None,
            "findings": [],
            "observed": None,
        }
        record.update(fields)
        self.lines.append(json.dumps(record))

    def result(self, tool_use_id: str, *, rejected: object = False) -> None:
        """Add one tool result; ``rejected`` is written as is_error unless it is False."""
        self.results.append((tool_use_id, rejected))

    def write(self) -> str:
        """Write the log and the transcript, and return the log path."""
        self.case.put("gate.jsonl", "\n".join(self.lines) + "\n")
        if self.results:
            lines = [json.dumps({"type": "summary", "note": "not a tool result"})]
            for tool_use_id, rejected in self.results:
                item: Json = {"type": "tool_result", "tool_use_id": tool_use_id, "content": "x"}
                if rejected is not False:
                    item["is_error"] = rejected
                lines.append(json.dumps({"type": "user", "message": {"content": [item]}}))
            self.case.put("t1.jsonl", "\n".join(lines) + "\n")
        self.case.log = self.case.sandbox / "gate.jsonl"
        return str(self.case.log)

    def report(self, *args: str) -> None:
        """Write the trial and run the report on its log with ``args``."""
        self.case.run(self.write(), *args)


Builder = Callable[[Trial], None]


def run_case(name: str, build: Builder) -> Json:
    """Build and run one case in its own sandbox."""
    with sandbox() as path:
        case = Case(path)
        build(Trial(case))
        return case.fixture(name)


def classes_each(trial: Trial) -> None:
    trial.add("rejected", findings=[complexity()])
    trial.result("rejected", rejected=True)
    trial.add("no-transcript", transcript="absent", findings=[complexity()])
    trial.add("no-result", findings=[complexity()])
    trial.add("unobserved", session="s4", findings=[complexity()])
    trial.result("unobserved")
    for name, session, units, finding in (
        ("grew-then-shrank", "s5", [("big", 12)], complexity()),
        ("crossed", "s6", [], complexity(cc_old=9, cc_new=11)),
        ("still-grown", "s7", [("big", 13)], complexity()),
    ):
        trial.add(name, session=session, findings=[finding])
        trial.result(name)
        trial.add(f"later-{session}", session=session, outcome="clean", observed=observed(units))
        trial.result(f"later-{session}")
    trial.report()


def later(*, rejected: bool = False, **fields: Any) -> Builder:
    def build(trial: Trial) -> None:
        trial.add("finding", findings=[complexity()])
        trial.result("finding")
        trial.add("later", **fields)
        trial.result("later", rejected=rejected)
        trial.report()

    return build


def other_session_or_path(trial: Trial) -> None:
    trial.add("finding", findings=[complexity()])
    trial.result("finding")
    trial.add("other-session", session="s2", outcome="clean", observed=observed([]))
    trial.result("other-session")
    trial.add("other-path", path="/repo/b.py", outcome="clean", observed=observed([]))
    trial.result("other-path")
    trial.report()


def contract_multiset(at_finding: list[Any] | None, after: list[Any]) -> Builder:
    def build(trial: Trial) -> None:
        at = None if at_finding is None else observed(keys=at_finding)
        trial.add("finding", findings=[contract()], observed=at)
        trial.result("finding")
        trial.add("later", outcome="clean", observed=observed(keys=after))
        trial.result("later")
        trial.report()

    return build


def clone_multiset(at_finding: list[str] | None, after: list[str], **where: str) -> Builder:
    def build(trial: Trial) -> None:
        at = None if at_finding is None else copies(*at_finding)
        trial.add("finding", findings=[clone()], observed=at)
        trial.result("finding")
        trial.add("later", outcome="clean", observed=copies(*after, **where))
        trial.result("later")
        trial.report()

    return build


def another_path_row(trial: Trial) -> None:
    trial.add("unit", session="s1", findings=[complexity()])
    trial.result("unit")
    moved = observed([("big", 13)], path="pkg/other.py")
    trial.add("unit-later", session="s1", outcome="clean", observed=moved)
    trial.result("unit-later")
    trial.add("hit", session="s2", findings=[contract()], observed=observed(keys=[KEY]))
    trial.result("hit")
    moved = observed(keys=[KEY], path="pkg/other.py")
    trial.add("hit-later", session="s2", outcome="clean", observed=moved)
    trial.result("hit-later")
    trial.report()


def figures(cc_values: tuple[int, ...]) -> Builder:
    def build(trial: Trial) -> None:
        for index, cc in enumerate(cc_values):
            session = f"s{index}"
            trial.add(f"finding-{index}", session=session, findings=[complexity()])
            trial.result(f"finding-{index}")
            state = observed([("big", cc)])
            trial.add(f"later-{index}", session=session, outcome="clean", observed=state)
            trial.result(f"later-{index}")
        trial.report()

    return build


def partial_transcript(trial: Trial) -> None:
    trial.add("finding", findings=[complexity()])
    trial.result("finding")
    log = trial.write()
    text = (trial.case.sandbox / "t1.jsonl").read_text(encoding="utf-8")
    trial.case.put("t1.jsonl", text + '{"type": "user", "mess')
    trial.case.run(log)


def sampled(trial: Trial, complexity_findings: int = 40, contract_findings: int = 5) -> str:
    for index in range(complexity_findings):
        trial.add(f"c{index}", findings=[complexity()])
        trial.result(f"c{index}")
    for index in range(contract_findings):
        trial.add(f"k{index}", findings=[contract()])
        trial.result(f"k{index}")
    trial.add("rejected", findings=[complexity()])
    trial.result("rejected", rejected=True)
    return trial.write()


def same_seed(trial: Trial) -> None:
    log = sampled(trial)
    for out, seed in (("a.csv", "0"), ("b.csv", "0"), ("c.csv", "7")):
        trial.case.run(log, "--sample-out", trial.case.path(out), "--seed", seed)


def hash_seed(trial: Trial) -> None:
    log = sampled(trial)
    drawn = []
    for value in ("1", "2"):
        out = trial.case.sandbox / f"hash-{value}.csv"
        subprocess.run(
            [sys.executable, str(REPORT_PATH), log, "--sample-out", str(out)],
            env={"PATH": os.environ["PATH"], "PYTHONHASHSEED": value},
            capture_output=True,
            check=True,
            timeout=HOOK_SECONDS,
        )
        drawn.append(out.read_bytes())
        out.unlink()
    if drawn[0] != drawn[1]:
        message = "refused: the reference draw depends on the hash seed"
        raise SystemExit(message)
    trial.case.run(log, "--sample-out", trial.case.path("hash.csv"))


def existing_sample(trial: Trial) -> None:
    log = sampled(trial)
    trial.case.put("labels.csv", "filled\n")
    trial.case.run(log, "--sample-out", trial.case.path("labels.csv"))


def seeds(trial: Trial) -> None:
    log = sampled(trial, 35, 0)
    for index, seed in enumerate(("5", "-3", "7", "10", "0")):
        trial.case.run(log, "--sample-out", trial.case.path(f"seed{index}.csv"), "--seed", seed)


def rows_of(path: Path) -> Rows:
    with path.open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def rewritten(rows: Rows) -> str:
    text = io.StringIO()
    writer = csv.DictWriter(text, fieldnames=report.CSV_FIELDS)
    writer.writeheader()
    writer.writerows(rows)
    return text.getvalue()


def labeled(text_of: Callable[[Rows], str | bytes], count: int = 50) -> Builder:
    """Sample ``count`` applied findings, then score the labels file that ``text_of`` makes."""

    def build(trial: Trial) -> None:
        for index in range(count):
            trial.add(f"c{index}", findings=[complexity()])
            trial.result(f"c{index}")
        log = trial.write()
        case = trial.case
        case.run(log, "--sample-out", case.path("sample.csv"))
        case.put("labels.csv", text_of(rows_of(case.sandbox / "sample.csv")))
        case.run(log, "--labels", case.path("labels.csv"))

    return build


def with_labels(labels: list[str], change: dict[str, str] | None = None) -> Callable[[Rows], str]:
    def text_of(rows: Rows) -> str:
        for row, label in zip(rows, labels, strict=False):
            row["label"] = label
        if change is not None:
            rows[3].update(change)
        return rewritten(rows)

    return text_of


def duplicated(rows: Rows) -> str:
    lines = rewritten([{**row, "label": "correct"} for row in rows]).splitlines()
    return "\n".join([*lines, lines[1]]) + "\n"


def blank_rows(rows: Rows) -> str:
    lines = rewritten([{**row, "label": "correct"} for row in rows]).split("\r\n")
    return "\r\n\r\n".join(lines[:4]) + "\r\n" + "\r\n".join(lines[4:])


def short_row(rows: Rows) -> str:
    return rewritten(rows) + "c1#0,complexity_delta\r\n"


def extra_columns(rows: Rows) -> str:
    body = "".join(f"{row['finding_id']},{row['check']},wrong,correct,x,y\r\n" for row in rows[:3])
    return "finding_id,check,label,label,extra\r\n" + body


def quoted_newline_then_error(rows: Rows) -> str:
    rows[1]["message"] = "two\r\nlines"
    rows[2]["label"] = "maybe"
    return rewritten(rows)


def eof_in_quotes(rows: Rows) -> str:
    return rewritten(rows[:2]) + '"' + rows[2]["finding_id"]


def chunk_edge(ending: str) -> Callable[[Rows], str]:
    """Return a labels text whose row ending starts at the last byte of the first chunk."""

    def text_of(rows: Rows) -> str:
        head = rewritten(rows[:3])
        start = f"{rows[3]['finding_id']},complexity_delta,,,,,,,"
        pad = "x" * (CHUNK - 1 - len((head + start).encode()))
        tail = rewritten(rows[4:6]).split("\r\n", 1)[1]
        return head + start + pad + ending + tail

    return text_of


def bad_byte(where: int) -> Callable[[Rows], bytes]:
    """Return a labels text with a byte that is no UTF-8 at ``where``, inside a long message."""

    def text_of(rows: Rows) -> bytes:
        rows[1]["message"] = "x" * 12000
        data = rewritten(rows).encode()
        return data[:where] + b"\xe9" + data[where:]

    return text_of


def field_width(width: int) -> Callable[[Rows], str]:
    def text_of(rows: Rows) -> str:
        rows[1]["message"] = "x" * width
        return rewritten(rows)

    return text_of


def special_fields(trial: Trial) -> None:
    texts = ["a,b", 'say "hi"', "one\rtwo", "one\ntwo", "one\r\ntwo", ""]
    for index, text in enumerate(texts):
        finding = complexity()
        finding["message"] = text
        finding["evidence"] = f'"{text}"'
        if index % 2:
            finding["detail"]["unit"] = "u,1"
        trial.add(f"c{index}", findings=[finding])
        trial.result(f"c{index}")
    log = trial.write()
    trial.case.run(log, "--sample-out", trial.case.path("sample.csv"), "--seed", "5")
    trial.case.run(log, "--labels", trial.case.path("sample.csv"))


def malformed_line(line: str) -> Builder:
    def build(trial: Trial) -> None:
        trial.add("fine", outcome="skipped", reason="every check is off")
        trial.lines.append(line)
        trial.report()

    return build


def malformed_detail(make: Callable[[], Json], **change: object) -> Builder:
    def build(trial: Trial) -> None:
        finding = make()
        for key, value in change.items():
            if value is DELETE:
                del finding["detail"][key]
            else:
                finding["detail"][key] = value
        trial.add("finding", findings=[finding])
        trial.report()

    return build


def with_record(**fields: Any) -> Builder:
    def build(trial: Trial) -> None:
        trial.add("finding", **fields)
        trial.report()

    return build


def transcript_place(*, directory: bool) -> Builder:
    def build(trial: Trial) -> None:
        case = trial.case
        if directory:
            case.mkdir("t1.jsonl")
        else:
            case.put("plain", "a file\n")
        trial.add("finding", transcript="t1" if directory else "plain/t1", findings=[complexity()])
        case.put("gate.jsonl", "\n".join(trial.lines) + "\n")
        case.log = case.sandbox / "gate.jsonl"
        case.run(str(case.log))

    return build


def unknown_checks(trial: Trial) -> None:
    for index, check in enumerate(("zz_custom", "aa_other", "zz_custom")):
        trial.add(f"u{index}", findings=[{**contract(), "check": check, "detail": None}])
        trial.result(f"u{index}")
    trial.add("u3", findings=[contract()])
    trial.result("u3")
    trial.report("--sample-out", trial.case.path("sample.csv"))


def reasons_and_binaries(trial: Trial) -> None:
    reasons = ["b", "a", "b", "c", "", "d", "e", "f", "a", "g", None]
    for index, reason in enumerate(reasons):
        binary = [None, "", "/opt/one", "/opt/two"][index % 4]
        trial.add(f"d{index}", outcome="degraded", reason=reason, binary=binary)
    trial.report()


def is_error(value: object) -> Builder:
    def build(trial: Trial) -> None:
        trial.add("finding", findings=[complexity()])
        trial.result("finding", rejected=value)
        trial.report()

    return build


def record_line(**fields: object) -> bytes:
    record: Json = {
        "v": 1,
        "ts": 1791600000.0,
        "session_id": "s1",
        "transcript_path": "<T>/t1.jsonl",
        "tool_use_id": "toolu_1",
        "path": "/repo/pkg/app.py",
        "binary": "/opt/agentless-mcp",
        "outcome": "advisory",
        "reason": None,
        "findings": [complexity()],
        "observed": None,
    }
    record.update(fields)
    return json.dumps(record).encode()


TS = b'"ts": 1791600000.0'


def raw_log(text: bytes, transcript: bytes | None = None) -> Builder:
    def build(trial: Trial) -> None:
        case = trial.case
        case.put("gate.jsonl", text.replace(b"<T>", str(case.sandbox).encode()))
        if transcript is not None:
            case.put("t1.jsonl", transcript)
        case.log = case.sandbox / "gate.jsonl"
        case.run(str(case.log))

    return build


def result_line(tool_use_id: bytes) -> bytes:
    item = b'{"type": "tool_result", "tool_use_id": "' + tool_use_id + b'"}'
    return b'{"type": "user", "message": {"content": [' + item + b"]}}\n"


DEEP = b"[" * 10000 + b"]" * 10000


def deviation(name: str, original: dict[str, bytes], adjusted: dict[str, bytes]) -> Builder:
    """A case read by a ledger deviation: the Rust build must equal the adjusted input's run."""

    def build(trial: Trial) -> None:
        case = trial.case
        case.deviation = name
        place = str(case.sandbox).encode()
        args = [case.path("gate.jsonl"), "--sample-out", case.path("sample.csv")]
        for rel, data in original.items():
            (case.sandbox / rel).write_bytes(data.replace(b"<T>", place))
        out, err, code, crash = captured(lambda: report.main(args))
        case.reference = [case.shown(out), case.shown(err), code, crash]
        (case.sandbox / "sample.csv").unlink(missing_ok=True)
        for rel, data in original.items():
            case.put(rel, data.replace(b"<T>", place))
        for rel, data in adjusted.items():
            (case.sandbox / rel).write_bytes(data.replace(b"<T>", place))
        case.run(*args)
        for rel in adjusted:
            (case.sandbox / rel).write_bytes(case.put_files[rel])
        case.log = case.sandbox / "gate.jsonl"

    return build


def surrogate_log(text: str) -> bytes:
    finding = complexity()
    finding["message"] = text
    first = record_line(findings=[finding])
    second = record_line(outcome="degraded", findings=[], reason=text, tool_use_id="d")
    return first + b"\n" + second + b"\n"


def record_cases() -> list[tuple[str, Builder]]:
    """Return the cases of the reference tests' classes, figures, samples and labels."""
    rejected_later = later(outcome="clean", observed=observed([]), rejected=True)
    renamed = ("get-default-empty", "parse_response", KEY[2])
    bad = with_labels(["correct"] * 30, {"label": "maybe"})
    return [
        ("each_finding_lands_in_its_class", classes_each),
        ("a_later_record_without_observed_state", later(outcome="degraded", reason="timed out")),
        ("a_later_rejected_call_is_not_evidence", rejected_later),
        ("another_session_or_path_is_not_evidence", other_session_or_path),
        ("contract_two_then_one", contract_multiset([KEY, KEY], [KEY])),
        ("contract_two_then_two", contract_multiset([KEY, KEY], [KEY, KEY])),
        ("contract_none_then_zero", contract_multiset(None, [])),
        ("contract_none_then_one", contract_multiset(None, [KEY])),
        ("a_renamed_function_keeps_its_contract_hit", contract_multiset([KEY], [renamed])),
        ("a_row_for_another_path_is_not_this_findings_row", another_path_row),
        ("clone_two_then_one", clone_multiset([COPY, COPY], [COPY])),
        ("clone_two_then_none", clone_multiset([COPY, COPY], [])),
        ("clone_two_then_two", clone_multiset([COPY, COPY], [COPY, COPY])),
        ("clone_two_then_two_and_other", clone_multiset([COPY, COPY], [COPY, COPY, OTHER])),
        ("clone_none_then_none", clone_multiset(None, [])),
        ("clone_none_then_one", clone_multiset(None, [COPY])),
        ("clone_in_another_file", clone_multiset([COPY, COPY], [COPY, COPY], path="pkg/b.py")),
        ("a_later_record_without_clone_instances", later(outcome="clean", observed=observed([]))),
        ("the_report_prints_the_classes_and_the_rate", figures((12, 13, 12))),
        ("a_rate_on_a_tie", figures((13, 12, 12, 12, 12, 12, 12, 12))),
        ("no_observed_finding_gives_no_rate", later(outcome="skipped")),
        ("a_partial_transcript_line_is_counted", partial_transcript),
        ("the_same_seed_draws_the_same_findings", same_seed),
        ("the_draw_does_not_depend_on_the_hash_seed", hash_seed),
        ("an_existing_sample_file_is_never_overwritten", existing_sample),
        ("seed_values", seeds),
        ("27_of_30_correct_qualifies", labeled(with_labels(["correct"] * 27 + ["wrong"] * 3))),
        ("an_unlabeled_row_keeps_the_check_out", labeled(with_labels(["correct"] * 29))),
        ("label_maybe", labeled(bad)),
        ("label_unknown_id", labeled(with_labels([], {"finding_id": "toolu_unknown#0"}))),
        ("label_other_check", labeled(with_labels([], {"check": "contract_drift"}))),
        ("label_spaced", labeled(with_labels([" correct "] * 30))),
        ("a_duplicate_row_stops_the_report", labeled(duplicated)),
        ("labels_blank_rows", labeled(blank_rows, 12)),
        ("labels_short_row", labeled(short_row, 12)),
        ("labels_extra_columns", labeled(extra_columns, 12)),
        ("labels_quoted_newline_then_error", labeled(quoted_newline_then_error, 12)),
        ("labels_eof_in_quotes", labeled(eof_in_quotes, 12)),
        ("labels_empty", labeled(lambda _rows: "", 12)),
        ("labels_header_only", labeled(lambda _rows: "finding_id,check,label\r\n", 12)),
        ("labels_header_lacks", labeled(lambda _rows: "finding_id,check\r\n", 12)),
        ("labels_bom", labeled(lambda rows: BOM + rewritten(rows), 12)),
        ("labels_crlf_at_chunk_edge", labeled(chunk_edge("\r\n"), 12)),
        ("labels_cr_at_chunk_edge", labeled(chunk_edge("\r"), 12)),
        ("labels_lf_at_chunk_edge", labeled(chunk_edge("\n"), 12)),
        ("labels_bad_byte_in_first_chunk", labeled(bad_byte(300), 12)),
        ("labels_bad_byte_in_second_chunk", labeled(bad_byte(10000), 12)),
        ("special_csv_fields", special_fields),
        ("a_transcript_that_is_a_directory", transcript_place(directory=True)),
        ("a_transcript_below_a_file", transcript_place(directory=False)),
        ("unknown_checks", unknown_checks),
        ("degraded_reasons_and_binaries", reasons_and_binaries),
        ("is_error_zero", is_error(0)),
        ("is_error_null", is_error(None)),
        ("is_error_text", is_error("x")),
        ("is_error_nested", is_error([1, {"a": None, "b": 1.5e16}, True, -0.0])),
    ]


def malformed_cases() -> list[tuple[str, Builder]]:
    """Return the cases of malformed log lines, observed rows and finding details."""
    changed = {
        "path": "a.py",
        "unit": "f",
        "line": 1,
        "match": "name",
        "cc_old": 1,
        "cc_new": "2",
        "sloc_old": 1,
        "sloc_new": 2,
    }
    pathless = {"units_over_threshold": [{"unit": "big", "line": 1, "cc": 13, "sloc": 2}]}
    texty_site = [{"path": "a.py", "line": "1", "end_line": 2}]
    keyless = {"clone_instances": [{"path": "a.py"}]}
    return [
        ("not_json", malformed_line("not json")),
        ("log_version_2", malformed_line('{"v": 2}')),
        ("observed_missing", malformed_line('{"v": 1, "outcome": "advisory"}')),
        ("outcome_lost", malformed_line('{"v": 1, "outcome": "lost"}')),
        ("units_not_list", with_record(outcome="clean", observed={"units_over_threshold": None})),
        ("unit_path_missing", with_record(outcome="clean", observed=pathless)),
        ("contract_row_list", with_record(outcome="clean", observed={"contract_keys": [[1]]})),
        ("units_changed_text", with_record(outcome="clean", observed={"units_changed": [changed]})),
        ("clone_instance_key_missing", with_record(outcome="clean", observed=keyless)),
        ("clone_key_missing", malformed_detail(clone, key=DELETE)),
        ("clone_sites_not_list", malformed_detail(clone, sites={})),
        ("clone_site_not_object", malformed_detail(clone, sites=[[1, 2]])),
        ("clone_site_line_text", malformed_detail(clone, sites=texty_site)),
        ("clone_sites_total_null", malformed_detail(clone, sites_total=None)),
        ("complexity_threshold_missing", malformed_detail(complexity, threshold=DELETE)),
        ("complexity_match_unknown", malformed_detail(complexity, match="it's")),
    ]


def raw_cases() -> list[tuple[str, Builder]]:
    """Return the cases of hand-shaped log and transcript bytes, and the deviation cases."""
    # A JSON error names a position, so no sandbox path may stand before it.
    separated = record_line(reason="a" + LINE_SEPARATOR + "b", transcript_path="/t1.jsonl")
    separated = separated.replace(b"\\u2028", LINE_SEPARATOR.encode())
    two_times = record_line(ts=1791600059) + b"\n"
    two_times += record_line(ts=-62135596800, tool_use_id="t2") + b"\n"
    edges = b"\xff\xfe junk\n" + result_line(b"toolu_1\xff") + result_line(b"toolu_1") + b"[1, 2\n"
    idless = b'{"message": {"content": [{"type": "tool_result"}]}}\n'
    deep = {"gate.jsonl": record_line() + b"\n", "t1.jsonl": DEEP + b"\n" + result_line(b"toolu_1")}
    surrogates = {
        "gate.jsonl": surrogate_log(chr(0xDCFF) + chr(0xD800)),
        "t1.jsonl": result_line(b"toolu_1"),
    }
    detail_null = record_line(findings=[{**complexity(), "detail": None}])
    return [
        ("log_bom", raw_log(b"\xef\xbb\xbf" + record_line() + b"\n")),
        ("log_blank_lines", raw_log(b"\n \t\n" + record_line() + b"\n\x0c\n")),
        ("log_line_separator", raw_log(separated + b"\n")),
        ("log_duplicate_keys", raw_log(b'{"v": 2, ' + record_line()[1:] + b"\n")),
        ("log_v_true", raw_log(record_line(v=True) + b"\n")),
        ("log_v_float", raw_log(record_line(v=1.0) + b"\n")),
        ("log_v_minus_zero", raw_log(record_line().replace(b'"v": 1', b'"v": -0') + b"\n")),
        ("log_not_an_object", raw_log(b"[1]\n")),
        ("log_finding_not_object", raw_log(record_line(findings=[1]) + b"\n")),
        ("log_severity_bad", raw_log(record_line(findings=[complexity(severity="it's")]) + b"\n")),
        ("log_outcome_quote", raw_log(record_line(outcome='it"s') + b"\n")),
        ("log_detail_null", raw_log(detail_null + b"\n")),
        ("log_identity_null", raw_log(record_line(session_id=None) + b"\n")),
        ("log_not_utf8", raw_log(b"{}\n\xff\n" + record_line() + b"\n")),
        ("log_huge_int_ts", raw_log(record_line().replace(TS, b'"ts": 1' + b"0" * 400) + b"\n")),
        ("log_nan_ts", raw_log(record_line().replace(TS, b'"ts": NaN') + b"\n")),
        ("log_inf_ts", raw_log(record_line().replace(TS, b'"ts": 1e400') + b"\n")),
        ("log_year_10000", raw_log(record_line(ts=253402300800) + b"\n")),
        ("log_deep", raw_log(DEEP + b"\n")),
        ("log_int_ts", raw_log(two_times)),
        ("transcript_edges", raw_log(record_line() + b"\n", edges)),
        ("transcript_id_missing", raw_log(record_line() + b"\n", idless)),
        (
            "transcript_nesting",
            deviation(
                "a transcript line nested past the depth limit counts as unreadable",
                deep,
                {"t1.jsonl": b"[\n" + result_line(b"toolu_1")},
            ),
        ),
        (
            "lone_surrogate_output",
            deviation(
                "a lone surrogate reaches stdout and the labeling file as U+FFFD",
                surrogates,
                {"gate.jsonl": surrogate_log(chr(0xFFFD) + chr(0xFFFD))},
            ),
        ),
    ]


def report_cases() -> list[object]:
    """Return every report case: the reference test nodes, then the untested inputs."""
    builders = record_cases() + malformed_cases() + raw_cases()
    return [run_case(name, build) for name, build in builders]


def set_branch(trial: Trial) -> None:
    log = sampled(trial, 290, 0)
    trial.case.run(log, "--sample-out", trial.case.path("sample.csv"), "--seed", "11")


def large_cases() -> list[object]:
    """Return the cases whose inputs are large: the set branch and the field limit."""
    builders: list[tuple[str, Builder]] = [
        ("the_set_branch_draws_from_a_large_population", set_branch),
        ("a_field_at_the_limit", labeled(field_width(FIELD_LIMIT), 12)),
        ("a_field_past_the_limit", labeled(field_width(FIELD_LIMIT + 1), 12)),
    ]
    return [run_case(name, build) for name, build in builders]


def bits_cases() -> list[object]:
    seeds = ["0:complexity_delta", "7:contract_drift", "-3:clone_delta", "5:zz", ":", "x" * 100]
    widths = [1, 2, 5, 16, 31, 32, 33, 48, 63, 64]
    cases: list[object] = []
    for seed in seeds:
        generator = random.Random(seed)
        cases.append([seed, [[k, generator.getrandbits(k)] for k in widths for _ in range(3)]])
    return cases


def below_cases() -> list[object]:
    bounds = [1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 30, 255, 256, 257, 277, 278, 300, 1000]
    bounds += [2**31 - 1, 2**31, 2**32 - 1, 2**32, 2**32 + 1, 2**40 + 3, 2**63, 2**64 - 1]
    generator = cast("Any", random.Random("0:clone_delta"))
    return [[n, [int(generator._randbelow(n)) for _ in range(5)]] for n in bounds]


def set_size(k: int) -> int:
    """The reference's population bound of the pool draw, by its own expression."""
    size = 21
    if k > SMALL_SAMPLE:
        size += 4 ** math.ceil(math.log(k * 3, 4))
    return size


def sample_cases() -> list[object]:
    cases: list[object] = []
    for seed in ("0:complexity_delta", "3:x"):
        sizes = [(n, min(SAMPLE_SIZE, n)) for n in (0, 1, 2, 5, 6, 29, 30, 31, 40, 276, 277, 278)]
        sizes += [(n, SAMPLE_SIZE) for n in (279, 280, 300, 1000)]
        sizes += [(300, k) for k in range(SAMPLE_SIZE + 1)]
        sizes += [(set_size(k) + d, k) for k in range(6, SAMPLE_SIZE + 1) for d in (0, 1)]
        cases += [[seed, n, k, random.Random(seed).sample(range(n), k)] for n, k in sizes]
    return cases


def when_cases() -> list[object]:
    values = [0.0, 59.9999994, 59.9999995, -0.5, -1e-7, 1791600000.0, 1.5, 86399.9999996]
    values += [-62135596800.0, -62135596800.5, 253402300799.0, 253402300799.9999995]
    values += [253402300800.0, float("nan"), float("inf"), float("-inf"), 1e17, -1e17, 1e19]
    values += [9.2e18, 2.0**63, -(2.0**63), -86400.0, -30000000000.0, 1e-300, 4e12, -61e9]
    cases: list[object] = []
    for value in values:
        _out, _err, text, crash = captured(functools.partial(report._when, value))
        bits = struct.unpack("<Q", struct.pack("<d", value))[0]
        cases.append([f"0x{bits:016x}", text if crash is None else None, crash])
    return cases


def csv_read(data: bytes) -> Json:
    """Return what the reference reads from a file: its header, its rows, and the stopping error."""
    with tempfile.TemporaryDirectory(prefix="gate-report-csv-") as made:
        path = Path(made) / "x.csv"
        path.write_bytes(data)
        rows: list[object] = []
        fieldnames = None
        try:
            with path.open(newline="", encoding="utf-8") as handle:
                reader = csv.DictReader(handle)
                fieldnames = reader.fieldnames
                for row in reader:
                    rows.append([reader.line_num, [[key, value] for key, value in row.items()]])
        except (UnicodeDecodeError, csv.Error) as error:
            return {"fieldnames": fieldnames, "rows": rows, "error": str(error)}
    return {"fieldnames": fieldnames, "rows": rows, "error": None}


def csv_read_cases() -> list[object]:
    inputs = [
        b"",
        b"\r\n",
        b"a,b\r\n1,2\r\n",
        b"a,b\n1,2\n\n\n3\n",
        b'a,b\r\n"x\r\ny",2\r\n"q""q",3\r\n',
        b'a,b\r\n"open,1\r\n',
        b'a,b\r\n"x"y,2\r\n',
        b"a,a,b\r\n1,2,3,4\r\n5\r\n",
        b"a,b\r1,2\r3,4",
        b"\xef\xbb\xbfa,b\r\n1,2\r\n",
        b"a,b\r\n1,\xff\r\n",
        b"a,b\r\n1,\xe2\x82\r\n",
        b"a,b\r\n1,\xe2\x82",
        b'a,b\r\n"' + b"x" * FIELD_LIMIT + b'",1\r\n',
        b"a,b\r\n" + b"x" * (FIELD_LIMIT + 1) + b"\r\n",
        b"a,b\r\n" + b"y" * 8185 + b"\r\n1,2\r\n",
        b"a,b\r\n" + b"y" * 8186 + b"\r\n1,2\r\n",
        b"a,b\r\n" + b"y" * 8186 + b"\xe9\r\n",
        b"a,b\r\n" + b"y" * 8180 + EURO.encode() + b"\r\n1,2\r\n",
        b"a,b\r\n,\r\n\x00,\x00\r\n",
        b" a , b \r\n x , y \r\n",
    ]
    return [[latin(data), csv_read(data)] for data in inputs]


def csv_write_cases() -> list[object]:
    rows = [
        ["a", "b", "", "c", "d", "1", "", "e", "f"],
        ["a,b", 'q"q', "", "one\rtwo", "one\ntwo", "-5", "x\r\ny", " lead", "trail "],
        ["", "", "", "", "", "", "", "", ""],
        ['"', ",", "\r", "\n", EURO, LINE_SEPARATOR, "\t", "'", ";"],
    ]
    cases: list[object] = []
    for row in rows:
        text = io.StringIO()
        csv.writer(text).writerow(row)
        cases.append([row, latin(text.getvalue().encode())])
    return cases


def verdict_cases() -> list[object]:
    counts = [None, (27, 3, 0), (30, 0, 0), (26, 4, 0), (0, 0, 0), (29, 0, 1), (9, 1, 0), (1, 7, 0)]
    cases: list[object] = []
    for applied in (0, 49, 50, 51, 300):
        for count in counts:
            labels = None if count is None else report.LabelCount(*count)
            shown = None if count is None else list(count)
            cases.append([applied, shown, report.verdict(applied, labels)])
    return cases


def stub_session(path: Path) -> list[str]:
    """Run the reference hook with a stub lint on two edits; return its log with pinned times."""
    root = path / "repo"
    (root / "pkg").mkdir(parents=True)
    branches = "".join(f"    if x == {i}:\n        return {i}\n" for i in range(11))
    (root / "pkg" / "app.py").write_text("def big(x):\n" + branches + "    return -1\n")
    for step in (["init", "-q"], ["add", "-A"], ["commit", "-q", "-m", "setup"]):
        run_git(root, IDENTITY, *step)
    stub = path / "agentless-mcp"
    stub.write_text(STUB.format(python=sys.executable), encoding="utf-8")
    stub.chmod(0o755)
    first_state = {**(observed([("big", 13)]) or {}), **copies(COPY, COPY)}
    second_state = {**(observed([("big", 12)]) or {}), **copies(COPY)}
    lints = [
        {"id": "pkg/app.py", "findings": [complexity(), clone()], "observed": first_state},
        {"id": "pkg/app.py", "findings": [], "observed": second_state},
    ]
    spec = [json.dumps({"candidates": [lint]}) for lint in lints]
    (path / "spec.json").write_text(json.dumps(spec), encoding="utf-8")
    env = {
        "PATH": str(Path(shutil.which("git") or "git").parent),
        "HOME": str(path),
        "TMPDIR": str(path),
        "GIT_CONFIG_GLOBAL": os.devnull,
        "AGENTLESS_EROSION_BIN": str(stub),
        "AGENTLESS_EROSION_LOG": str(path / "hook.jsonl"),
        "STUB_CALLS": str(path / "calls.txt"),
        "STUB_SPEC": str(path / "spec.json"),
    }
    for tool_use_id, old, new in (
        ("toolu_a", "    return -1\n", "    if x < 0:\n        return 0\n    return -1\n"),
        ("toolu_b", "    if x == 0:\n", "    if x == 100:\n"),
    ):
        payload = {
            "session_id": "s1",
            "transcript_path": str(path / "t1.jsonl"),
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
            [sys.executable, str(HOOK_PATH)],
            input=json.dumps(payload).encode(),
            env=env,
            capture_output=True,
            check=True,
            timeout=HOOK_SECONDS,
        )
    lines = []
    for index, line in enumerate((path / "hook.jsonl").read_text(encoding="utf-8").splitlines()):
        record = json.loads(line)
        # The hook reads the clock, so its two times are pinned; every other byte is the hook's.
        record["ts"] = 1791600000.0 + index
        record["latency_ms"] = 200.0
        lines.append(json.dumps(record))
    shutil.rmtree(root)
    for name in ("hook.jsonl", "calls.txt", "spec.json", "agentless-mcp"):
        (path / name).unlink()
    return lines


def hook_log_case() -> Json:
    """Report on a log that the reference hook wrote, as a trial that started under it."""
    with sandbox() as path:
        case = Case(path)
        lines = stub_session(path)
        case.put("gate.jsonl", "\n".join(lines) + "\n")
        case.put("t1.jsonl", result_line(b"toolu_a") + result_line(b"toolu_b"))
        case.log = path / "gate.jsonl"
        case.run(str(case.log), "--sample-out", case.path("sample.csv"))
        return case.fixture("the_report_reads_the_log_the_reference_hook_writes")


def parser_run(
    argv: list[str],
    columns: str | None,
    parser_of: Callable[[], argparse.ArgumentParser] | None = None,
) -> Json:
    """Run the reference parser, with the Rust program name, on argv at one terminal width."""
    saved = os.environ.get("COLUMNS")
    os.environ["COLUMNS"] = NO_TERMINAL_COLUMNS if columns is None else columns
    try:
        parser = (parser_of or report._parser)()
        parser.prog = RUST_PROG
        out, err, value, crash = captured(lambda: parser.parse_args(argv))
    finally:
        if saved is None:
            os.environ.pop("COLUMNS", None)
        else:
            os.environ["COLUMNS"] = saved
    if crash is not None:
        message = f"refused: the reference parser crashed on {argv}: {crash}"
        raise SystemExit(message)
    run: Json = {"argv": argv, "columns": columns, "out": out, "err": err, "parsed": None}
    if isinstance(value, argparse.Namespace):
        run["code"] = 0
        run["parsed"] = {
            "log": value.log,
            "sample_out": value.sample_out,
            "labels": value.labels,
            "seed": str(value.seed),
        }
    else:
        run["code"] = value
    return run


def ascii_int(text: str) -> int:
    """The deviation's integer rule: int() for a text whose digits are all ASCII."""
    if any(c.isdecimal() and not c.isascii() for c in text):
        raise ValueError(text)
    return int(text)


ascii_int.__name__ = "int"


def ascii_parser() -> argparse.ArgumentParser:
    """The reference parser with the deviation's integer rule for --seed."""
    parser = cast("argparse.ArgumentParser", report._parser())
    for action in parser._actions:
        if action.dest == "seed":
            action.type = ascii_int
    return parser


def parser_cases() -> list[object]:
    widths = (None, "20", "40", "60", "100")
    runs: list[object] = [parser_run(["--help"], width) for width in widths]
    runs.append(parser_run(["-h"], "200"))
    for argv in (
        [],
        ["a", "b"],
        ["log"],
        ["log", "--seed"],
        ["log", "--seed", "x"],
        ["log", "--seed", "+5"],
        ["log", "--seed", " 7 "],
        ["log", "--seed", "1_0"],
        ["log", "--seed", "-3"],
        ["log", "--seed=-3"],
        ["log", "--seed", "007"],
        ["log", "--seed", "-0"],
        ["log", "--seed", "1" * 4300],
        ["log", "--seed", "1" * 4301],
        ["log", "--seed", "1__0"],
        ["log", "--seed", EM_SPACE + "5" + EM_SPACE],
        ["--s", "1", "log"],
        ["--sam", "x.csv", "--lab", "y.csv", "log"],
        ["--labels"],
        ["--bogus", "log"],
        ["--", "-log"],
        ["-x"],
        ["log", "--sample-out", "a.csv", "--sample-out", "b.csv"],
    ):
        runs.append(parser_run(argv, None))
    for argv in (["log", "--seed", chr(0x665)], ["log", "--seed", "1" + chr(0xFF12)]):
        run = parser_run(argv, None, ascii_parser)
        run["deviation"] = "digits outside ASCII are refused"
        run["reference"] = parser_run(argv, None)["parsed"]
        runs.append(run)
    return runs


def build_files() -> dict[Path, bytes]:
    """Return every fixture file of the hook and cli crates."""
    units: Fixture = {
        "bits": bits_cases(),
        "below": below_cases(),
        "sample": sample_cases(),
        "when": when_cases(),
        "csv_read": csv_read_cases(),
        "csv_write": csv_write_cases(),
        "verdict": verdict_cases(),
    }
    return {
        HOOK_FIXTURES / "runs.json": render({"runs": report_cases()}),
        HOOK_FIXTURES / "large.json": render({"runs": large_cases()}),
        HOOK_FIXTURES / "hooklog.json": render({"runs": [hook_log_case()]}),
        HOOK_FIXTURES / "units.json": render(units),
        CLI_FIXTURES / "parser.json": render({"runs": parser_cases()}),
    }


def main() -> int:
    """Write the fixtures, or compare them with --check, and return the exit status."""
    return write_or_check_files(__doc__.splitlines()[0], HOOK_FIXTURES, build_files)


if __name__ == "__main__":
    sys.exit(main())
