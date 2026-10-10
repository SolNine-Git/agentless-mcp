#!/usr/bin/env python3
"""Turn the erosion gate log into trial figures, a labeling sample, and a flip verdict per check."""

from __future__ import annotations

import argparse
import csv
import json
import random
import sys
from collections import Counter
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

LOG_VERSION = 1
COMPLEXITY_DELTA = "complexity_delta"
CONTRACT_DRIFT = "contract_drift"
KNOWN_CHECKS = (COMPLEXITY_DELTA, CONTRACT_DRIFT)
MATCH_PASSES = ("body", "name", "collision", "new")
CONTRACT_KEY_FIELDS = ("rule", "unit", "capture")
OUTCOMES = ("clean", "advisory", "blocked", "degraded", "skipped")
FINDING_OUTCOMES = ("advisory", "blocked")
DEGRADED = "degraded"
FINDING_SEVERITIES = ("warning", "advisory")
NULL = type(None)

APPLIED = "applied"
REJECTED = "rejected"
NOT_APPLIED = "not applied"
UNRESOLVED = "unresolved"
UNOBSERVED = "unobserved"
ACTED_ON = "acted on"
NOT_ACTED_ON = "not acted on"
CLASSES = (NOT_APPLIED, UNRESOLVED, UNOBSERVED, ACTED_ON, NOT_ACTED_ON)
APPLIED_CLASSES = (UNOBSERVED, ACTED_ON, NOT_ACTED_ON)

SAMPLE_SIZE = 30
MIN_APPLIED = 50
# Precision at or above 9/10, compared in integers so 27 of 30 meets the bar exactly.
PRECISION_NUMERATOR = 9
PRECISION_DENOMINATOR = 10
DEFAULT_SEED = 0
CORRECT = "correct"
WRONG = "wrong"
LABELS = (CORRECT, WRONG, "")
CSV_FIELDS = (
    "finding_id",
    "check",
    "label",
    "session_id",
    "path",
    "line",
    "unit",
    "message",
    "evidence",
)
REQUIRED_LABEL_FIELDS = ("finding_id", "check", "label")
TOP_REASONS = 5

EXIT_OK = 0
EXIT_INPUT = 1
EXIT_USAGE = 2

CONDITION_3 = (
    "Flip condition 3 (no change to a check's rules or message since its trial started) "
    "is not in the log. Confirm it before you set a check's erosion_gate entry to block."
)


class ReportError(Exception):
    """A log, transcript, or label file that does not have the shape the report reads."""


@dataclass(frozen=True)
class UnitCC:
    """One unit over the threshold in a record's observed state."""

    unit: str
    line: int
    cc: int
    sloc: int


@dataclass(frozen=True)
class Observed:
    """The observed state of a proposed text: ``None`` for a list whose check did not run."""

    units_over_threshold: tuple[UnitCC, ...] | None
    contract_keys: tuple[tuple[str, str, str], ...] | None


@dataclass(frozen=True)
class Finding:
    """One finding the hook logged, with the numbers or key the report tracks it by."""

    check: str
    severity: str
    path: str
    line: int
    message: str
    evidence: str
    unit: str | None = None
    match: str | None = None
    cc_old: int | None = None
    threshold: int | None = None
    key: tuple[str, str, str] | None = None


@dataclass(frozen=True)
class Record:
    """One hook call from the log; ``line_no`` is its line in the log."""

    line_no: int
    ts: float
    session_id: str | None
    transcript_path: str | None
    tool_use_id: str | None
    path: str | None
    outcome: str
    reason: str | None
    binary: str | None
    findings: tuple[Finding, ...]
    observed: Observed | None


@dataclass(frozen=True)
class Transcript:
    """Each call's result in one transcript; ``results`` is ``None`` when the file is missing."""

    results: Mapping[str, str] | None
    unreadable_lines: int


@dataclass(frozen=True)
class Classified:
    """One finding with its D9 class and a stable id for the labeling file."""

    finding_id: str
    cls: str
    record: Record
    finding: Finding


@dataclass(frozen=True)
class LabelCount:
    """The labels one check received in a filled labeling file."""

    correct: int
    wrong: int
    unlabeled: int


def main(argv: Sequence[str] | None = None) -> int:
    """Print the trial report for one log, and write or score a labeling sample when asked."""
    args = _parser().parse_args(argv)
    try:
        records = read_log(Path(args.log))
        transcripts = read_transcripts(records)
        classified = classify(records, transcripts)
        labels = None if args.labels is None else read_labels(Path(args.labels), classified)
    except ReportError as error:
        sys.stderr.write(f"erosion_gate_report: {error}\n")
        return EXIT_INPUT
    notes: list[str] = []
    if args.sample_out is not None:
        try:
            written = write_sample(sample(classified, args.seed), Path(args.sample_out))
        except FileExistsError:
            sys.stderr.write(
                f"erosion_gate_report: {args.sample_out} exists. Name a new file, so that a "
                "filled labeling file is never overwritten.\n"
            )
            return EXIT_USAGE
        notes.append(f"Wrote {written} findings to label to {args.sample_out} (seed {args.seed}).")
    sys.stdout.write(report_text(records, transcripts, classified, labels, notes))
    return EXIT_OK


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="erosion_gate_report.py",
        description="Classify each erosion gate finding, print the trial figures per check, "
        "write a seeded labeling sample, and score a filled one against the flip criterion.",
    )
    parser.add_argument("log", help="the JSONL file that AGENTLESS_EROSION_LOG names")
    parser.add_argument(
        "--sample-out",
        metavar="FILE",
        help=f"write up to {SAMPLE_SIZE} applied findings per check to this new CSV file",
    )
    parser.add_argument(
        "--labels", metavar="FILE", help="score this filled CSV file (label: correct or wrong)"
    )
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED, help="the sample seed")
    return parser


def read_log(path: Path) -> list[Record]:
    """Parse every line of the hook log, or raise on the first line with another shape."""
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        message = f"cannot read {path}: {error}"
        raise ReportError(message) from error
    records = []
    for line_no, line in enumerate(text.splitlines(), 1):
        if not line.strip():
            continue
        where = f"{path}:{line_no}"
        try:
            document = json.loads(line)
        except ValueError as error:
            message = f"{where}: not JSON: {error}"
            raise ReportError(message) from error
        records.append(_record(document, line_no, where))
    return records


def _record(document: object, line_no: int, where: str) -> Record:
    if not isinstance(document, dict):
        message = f"{where}: not a JSON object"
        raise ReportError(message)
    version = _field(document, "v", int, where)
    if version != LOG_VERSION:
        message = f"{where}: log version {version}, not {LOG_VERSION}"
        raise ReportError(message)
    outcome = _field(document, "outcome", str, where)
    if outcome not in OUTCOMES:
        message = f"{where}: outcome {outcome!r} is not one of {', '.join(OUTCOMES)}"
        raise ReportError(message)
    observed_value = _field(document, "observed", (dict, NULL), where)
    observed = None if observed_value is None else _observed(observed_value, where)
    # A record that carries findings or an observed state must say whose call it was.
    identity = str if outcome in FINDING_OUTCOMES or observed is not None else (str, NULL)
    findings = _field(document, "findings", list, where)
    return Record(
        line_no=line_no,
        ts=float(_field(document, "ts", (int, float), where)),
        session_id=_field(document, "session_id", identity, where),
        transcript_path=_field(document, "transcript_path", identity, where),
        tool_use_id=_field(document, "tool_use_id", identity, where),
        path=_field(document, "path", identity, where),
        outcome=outcome,
        reason=_field(document, "reason", (str, NULL), where),
        binary=_field(document, "binary", (str, NULL), where),
        findings=tuple(
            _finding(item, f"{where}: findings[{index}]") for index, item in enumerate(findings)
        ),
        observed=observed,
    )


def _finding(item: object, where: str) -> Finding:
    if not isinstance(item, dict):
        message = f"{where} is not an object"
        raise ReportError(message)
    check = _field(item, "check", str, where)
    severity = _field(item, "severity", str, where)
    if severity not in FINDING_SEVERITIES:
        message = f"{where}: severity {severity!r} is not warning or advisory"
        raise ReportError(message)
    finding = Finding(
        check=check,
        severity=severity,
        path=_field(item, "path", str, where),
        line=_field(item, "line", int, where),
        message=_field(item, "message", str, where),
        evidence=_field(item, "evidence", str, where),
    )
    detail = _field(item, "detail", (dict, NULL), where)
    if check not in KNOWN_CHECKS:
        return finding
    if detail is None:
        message = f"{where}: detail is missing, and a {check} finding needs it"
        raise ReportError(message)
    if check == CONTRACT_DRIFT:
        return _with_key(finding, detail, f"{where}.detail")
    return _with_numbers(finding, detail, f"{where}.detail")


def _with_key(finding: Finding, detail: Mapping[str, object], where: str) -> Finding:
    rule, unit, capture = (_field(detail, name, str, where) for name in CONTRACT_KEY_FIELDS)
    return replace(finding, unit=unit, key=(rule, unit, capture))


def _with_numbers(finding: Finding, detail: Mapping[str, object], where: str) -> Finding:
    match = _field(detail, "match", str, where)
    if match not in MATCH_PASSES:
        message = f"{where}: match {match!r} is not one of {', '.join(MATCH_PASSES)}"
        raise ReportError(message)
    _field(detail, "cc_new", int, where)
    _field(detail, "sloc_new", int, where)
    _field(detail, "sloc_old", (int, NULL), where)
    return replace(
        finding,
        unit=_field(detail, "unit", str, where),
        match=match,
        cc_old=_field(detail, "cc_old", (int, NULL), where),
        threshold=_field(detail, "threshold", int, where),
    )


def _observed(value: Mapping[str, object], where: str) -> Observed:
    # A list lint did not send says the same as null: that check did not run on this file.
    units = _field(value, "units_over_threshold", (list, NULL), where, optional=True)
    keys = _field(value, "contract_keys", (list, NULL), where, optional=True)
    return Observed(
        units_over_threshold=None
        if units is None
        else tuple(
            _unit(item, f"{where}: units_over_threshold[{n}]") for n, item in enumerate(units)
        ),
        contract_keys=None
        if keys is None
        else tuple(_key(item, f"{where}: contract_keys[{n}]") for n, item in enumerate(keys)),
    )


def _unit(item: object, where: str) -> UnitCC:
    if not isinstance(item, dict):
        message = f"{where} is not an object"
        raise ReportError(message)
    return UnitCC(
        unit=_field(item, "unit", str, where),
        line=_field(item, "line", int, where),
        cc=_field(item, "cc", int, where),
        sloc=_field(item, "sloc", int, where),
    )


def _key(item: object, where: str) -> tuple[str, str, str]:
    if not isinstance(item, list) or len(item) != len(CONTRACT_KEY_FIELDS):
        message = f"{where} is not a [rule, unit, capture] list"
        raise ReportError(message)
    if not all(isinstance(part, str) for part in item):
        message = f"{where} holds a value that is not a string"
        raise ReportError(message)
    return (item[0], item[1], item[2])


def _field(
    document: Mapping[str, object],
    key: str,
    kind: type | tuple[type, ...],
    where: str,
    *,
    optional: bool = False,
) -> Any:
    if key not in document and optional:
        return None
    if key not in document:
        message = f"{where}: {key} is missing"
        raise ReportError(message)
    value = document[key]
    kinds = kind if isinstance(kind, tuple) else (kind,)
    if not isinstance(value, kinds) or (isinstance(value, bool) and bool not in kinds):
        names = " or ".join("null" if k is NULL else k.__name__ for k in kinds)
        message = f"{where}: {key} is not {names}"
        raise ReportError(message)
    return value


def read_transcripts(records: Iterable[Record]) -> dict[str, Transcript]:
    """Read each transcript the records name once, mapping each call to applied or rejected."""
    paths = {record.transcript_path for record in records if record.transcript_path is not None}
    return {path: _transcript(Path(path)) for path in sorted(paths)}


def _transcript(path: Path) -> Transcript:
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return Transcript(results=None, unreadable_lines=0)
    except OSError as error:
        message = f"cannot read the transcript {path}: {error}"
        raise ReportError(message) from error
    results: dict[str, str] = {}
    unreadable = 0
    for line_no, line in enumerate(text.splitlines(), 1):
        if not line.strip():
            continue
        document = _json_line(line)
        if document is None:
            unreadable += 1
            continue
        for item in _tool_results(document):
            tool_use_id, status = _result_status(item, f"{path}:{line_no}")
            results[tool_use_id] = status
    return Transcript(results=results, unreadable_lines=unreadable)


def _json_line(line: str) -> object | None:
    # A live session can leave a partial last line. The caller counts it and reports it.
    try:
        return json.loads(line)
    except ValueError:
        return None


def _tool_results(document: object) -> list[dict[str, object]]:
    # Most transcript lines carry no tool result, so a missing key here is not an error.
    if not isinstance(document, dict) or not isinstance(document.get("message"), dict):
        return []
    content = document["message"].get("content")
    if not isinstance(content, list):
        return []
    return [
        item for item in content if isinstance(item, dict) and item.get("type") == "tool_result"
    ]


def _result_status(item: Mapping[str, object], where: str) -> tuple[str, str]:
    tool_use_id = _field(item, "tool_use_id", str, where)
    # Claude Code writes no is_error key on a call that succeeded.
    if "is_error" not in item or item["is_error"] is False:
        return tool_use_id, APPLIED
    if item["is_error"] is True:
        return tool_use_id, REJECTED
    message = f"{where}: tool_result.is_error is {item['is_error']!r}, not a bool"
    raise ReportError(message)


def classify(records: Sequence[Record], transcripts: Mapping[str, Transcript]) -> list[Classified]:
    """Classify each logged finding as not applied, unresolved, unobserved, or acted on or not."""
    by_target: dict[tuple[str | None, str | None], list[Record]] = {}
    for record in records:
        if record.observed is not None:
            by_target.setdefault((record.session_id, record.path), []).append(record)
    classified = []
    for record in (record for record in records if record.outcome in FINDING_OUTCOMES):
        status = _status(record, transcripts)
        evidence = [
            later.observed
            for later in by_target.get((record.session_id, record.path), [])
            if later.line_no > record.line_no and _status(later, transcripts) == APPLIED
        ]
        for index, finding in enumerate(record.findings):
            cls = _class(finding, record.observed, status, evidence)
            classified.append(Classified(f"{record.tool_use_id}#{index}", cls, record, finding))
    return classified


def _status(record: Record, transcripts: Mapping[str, Transcript]) -> str:
    transcript = None if record.transcript_path is None else transcripts[record.transcript_path]
    if transcript is None or transcript.results is None:
        return UNRESOLVED
    if record.tool_use_id not in transcript.results:
        return UNRESOLVED
    return transcript.results[record.tool_use_id]


def _class(
    finding: Finding,
    at_finding: Observed | None,
    status: str,
    evidence: Sequence[Observed | None],
) -> str:
    if status == REJECTED:
        return NOT_APPLIED
    if status == UNRESOLVED:
        return UNRESOLVED
    seen = [
        observed for observed in evidence if observed is not None and _observes(observed, finding)
    ]
    if not seen:
        return UNOBSERVED
    if any(_acted(finding, at_finding, observed) for observed in seen):
        return ACTED_ON
    return NOT_ACTED_ON


def _observes(observed: Observed, finding: Finding) -> bool:
    if finding.check == COMPLEXITY_DELTA:
        return observed.units_over_threshold is not None
    if finding.check == CONTRACT_DRIFT:
        return observed.contract_keys is not None
    return False


def _acted(finding: Finding, at_finding: Observed | None, later: Observed) -> bool:
    if finding.check == COMPLEXITY_DELTA and later.units_over_threshold is not None:
        return _complexity_acted(finding, later.units_over_threshold)
    if finding.check == CONTRACT_DRIFT and later.contract_keys is not None:
        before = None if at_finding is None else at_finding.contract_keys
        return _contract_acted(finding, before, later.contract_keys)
    return False


def _complexity_acted(finding: Finding, over: Sequence[UnitCC]) -> bool:
    if finding.threshold is None:
        message = f"a {COMPLEXITY_DELTA} finding reached classification without its threshold"
        raise ReportError(message)
    # Growth is undone at the old cc. A crossing or a new unit is undone under the threshold.
    limit = finding.threshold
    if finding.cc_old is not None and finding.cc_old > finding.threshold:
        limit = finding.cc_old
    return not any(unit.unit == finding.unit and unit.cc > limit for unit in over)


def _contract_acted(
    finding: Finding,
    before: Sequence[tuple[str, str, str]] | None,
    after: Sequence[tuple[str, str, str]],
) -> bool:
    now = list(after).count(finding.key)
    if before is None:
        return now == 0
    return now < list(before).count(finding.key)


def sample(classified: Sequence[Classified], seed: int) -> list[Classified]:
    """Draw up to 30 applied findings per check; the same log and seed give the same rows."""
    drawn: list[Classified] = []
    for check in _checks(classified):
        population = [
            item
            for item in classified
            if item.finding.check == check and item.cls in APPLIED_CLASSES
        ]
        # A str seed hashes through SHA-512, so the draw does not depend on PYTHONHASHSEED.
        generator = random.Random(f"{seed}:{check}")
        drawn.extend(generator.sample(population, min(SAMPLE_SIZE, len(population))))
    return drawn


def write_sample(rows: Sequence[Classified], path: Path) -> int:
    """Write the rows to a new CSV file for labeling and return how many it holds."""
    with path.open("x", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=CSV_FIELDS)
        writer.writeheader()
        for item in rows:
            writer.writerow(
                {
                    "finding_id": item.finding_id,
                    "check": item.finding.check,
                    "label": "",
                    "session_id": item.record.session_id,
                    "path": item.finding.path,
                    "line": item.finding.line,
                    "unit": "" if item.finding.unit is None else item.finding.unit,
                    "message": item.finding.message,
                    "evidence": item.finding.evidence,
                }
            )
    return len(rows)


def read_labels(path: Path, classified: Sequence[Classified]) -> dict[str, LabelCount]:
    """Count the labels per check in a filled CSV file, or raise on a row the log cannot match."""
    applied = {item.finding_id: item for item in classified if item.cls in APPLIED_CLASSES}
    tally: Counter[tuple[str, str]] = Counter()
    try:
        with path.open(newline="", encoding="utf-8") as handle:
            reader = csv.DictReader(handle)
            _check_header(reader.fieldnames, path)
            seen: set[str] = set()
            for row in reader:
                tally[_label_row(row, applied, seen, f"{path}:{reader.line_num}")] += 1
    except (OSError, UnicodeDecodeError, csv.Error) as error:
        message = f"cannot read {path}: {error}"
        raise ReportError(message) from error
    return {
        check: LabelCount(tally[(check, CORRECT)], tally[(check, WRONG)], tally[(check, "")])
        for check in {check for check, _ in tally}
    }


def _check_header(fieldnames: Sequence[str] | None, path: Path) -> None:
    if fieldnames is None:
        message = f"{path} is empty"
        raise ReportError(message)
    missing = [name for name in REQUIRED_LABEL_FIELDS if name not in fieldnames]
    if missing:
        message = f"{path}: the header lacks {', '.join(missing)}"
        raise ReportError(message)


def _label_row(
    row: Mapping[str, str | None], applied: Mapping[str, Classified], seen: set[str], where: str
) -> tuple[str, str]:
    finding_id, check, label = (row[name] for name in REQUIRED_LABEL_FIELDS)
    if finding_id is None or check is None or label is None:
        message = f"{where}: the row has fewer columns than the header"
        raise ReportError(message)
    label = label.strip()
    if label not in LABELS:
        message = f"{where}: label {label!r} is not correct, wrong, or empty"
        raise ReportError(message)
    if finding_id in seen:
        message = f"{where}: {finding_id} appears twice"
        raise ReportError(message)
    seen.add(finding_id)
    if finding_id not in applied:
        message = f"{where}: {finding_id} is not an applied finding in the log"
        raise ReportError(message)
    logged = applied[finding_id].finding.check
    if logged != check:
        message = f"{where}: {finding_id} is a {logged} finding, not {check}"
        raise ReportError(message)
    return check, label


def verdict(applied: int, labels: LabelCount | None) -> str:
    """Return one check's flip verdict on conditions 1 and 2; condition 3 stays with the user."""
    if applied < MIN_APPLIED:
        return f"does not qualify: {applied} applied findings, under {MIN_APPLIED}."
    if labels is None:
        return (
            f"condition 1 holds ({applied} applied findings). "
            "Label a sample with --sample-out and --labels to test condition 2."
        )
    labeled = labels.correct + labels.wrong
    if labels.unlabeled or not labeled:
        total = labeled + labels.unlabeled
        return f"does not qualify: {labeled} of {total} sampled findings are labeled."
    precision = labels.correct / labeled
    if labels.correct * PRECISION_DENOMINATOR < labeled * PRECISION_NUMERATOR:
        return f"does not qualify: precision {precision:.2f}, under 0.90."
    return (
        f"qualifies on conditions 1 and 2 ({applied} applied findings, precision "
        f"{precision:.2f} over {labeled} labeled). Confirm condition 3 before you set "
        "erosion_gate to block."
    )


def report_text(
    records: Sequence[Record],
    transcripts: Mapping[str, Transcript],
    classified: Sequence[Classified],
    labels: Mapping[str, LabelCount] | None,
    notes: Sequence[str] = (),
) -> str:
    """Return the whole printed report."""
    lines = ["Erosion gate trial report", *_log_lines(records), *_transcript_lines(transcripts)]
    checks = _checks(classified)
    if not checks:
        lines.append("No findings in the log.")
    for check in checks:
        lines.append("")
        lines.extend(_check_lines(check, classified, labels))
    lines.extend(["", CONDITION_3, *notes])
    return "\n".join(lines) + "\n"


def _log_lines(records: Sequence[Record]) -> list[str]:
    if not records:
        return ["Log: 0 records."]
    return [
        f"Log: {len(records)} records, from {_when(records[0].ts)} to {_when(records[-1].ts)}.",
        _outcomes_line(records),
        *_degraded_reasons_line(records),
        *_binaries_line(records),
    ]


def _outcomes_line(records: Sequence[Record]) -> str:
    outcomes = Counter(record.outcome for record in records)
    return "Outcomes: " + ", ".join(f"{outcome} {outcomes[outcome]}" for outcome in OUTCOMES) + "."


def _degraded_reasons_line(records: Sequence[Record]) -> list[str]:
    reasons = Counter(
        record.reason for record in records if record.outcome == DEGRADED and record.reason
    )
    if not reasons:
        return []
    common = ", ".join(f"{reason} ({n})" for reason, n in reasons.most_common(TOP_REASONS))
    return [f"Commonest degraded reasons: {common}."]


def _binaries_line(records: Sequence[Record]) -> list[str]:
    binaries = Counter(record.binary for record in records if record.binary)
    if not binaries:
        return []
    seen = ", ".join(f"{binary} ({n} records)" for binary, n in binaries.items())
    return [f"Binaries: {seen}."]


def _transcript_lines(transcripts: Mapping[str, Transcript]) -> list[str]:
    missing = sum(1 for transcript in transcripts.values() if transcript.results is None)
    lines = [f"Transcripts: {len(transcripts)} named, {missing} missing."]
    lines.extend(
        f"Transcript {path} has {transcript.unreadable_lines} unreadable lines."
        for path, transcript in transcripts.items()
        if transcript.unreadable_lines
    )
    return lines


def _check_lines(
    check: str, classified: Sequence[Classified], labels: Mapping[str, LabelCount] | None
) -> list[str]:
    items = [item for item in classified if item.finding.check == check]
    counts = Counter(item.cls for item in items)
    applied = sum(counts[cls] for cls in APPLIED_CLASSES)
    count = _labels_for(check, labels)
    return [
        f"{check}: {len(items)} findings",
        "  " + ", ".join(f"{cls} {counts[cls]}" for cls in CLASSES),
        f"  applied {applied}",
        *_match_pass_line(check, items),
        _false_positive_line(counts),
        *_precision_line(count),
        f"  flip: {verdict(applied, count)}",
    ]


def _match_pass_line(check: str, items: Sequence[Classified]) -> list[str]:
    if check != COMPLEXITY_DELTA:
        return []
    passes = Counter(item.finding.match for item in items)
    return ["  match pass: " + ", ".join(f"{name} {passes[name]}" for name in MATCH_PASSES)]


def _false_positive_line(counts: Mapping[str, int]) -> str:
    observed = counts[ACTED_ON] + counts[NOT_ACTED_ON]
    rate = "n/a" if not observed else f"{counts[NOT_ACTED_ON] / observed:.2f}"
    return (
        f"  effective false-positive rate: {rate} ({counts[NOT_ACTED_ON]} of {observed} observed)"
    )


def _labels_for(check: str, labels: Mapping[str, LabelCount] | None) -> LabelCount | None:
    if labels is None:
        return None
    return labels[check] if check in labels else LabelCount(0, 0, 0)


def _precision_line(count: LabelCount | None) -> list[str]:
    if count is None:
        return []
    labeled = count.correct + count.wrong
    precision = "n/a" if not labeled else f"{count.correct / labeled:.2f}"
    total = labeled + count.unlabeled
    line = f"  labeled precision: {precision} ({count.correct} of {labeled} correct), "
    return [f"{line}{labeled} of {total} labeled"]


def _checks(classified: Iterable[Classified]) -> list[str]:
    present = {item.finding.check for item in classified}
    known = [check for check in KNOWN_CHECKS if check in present]
    return known + sorted(present - set(KNOWN_CHECKS))


def _when(ts: float) -> str:
    return datetime.fromtimestamp(ts, tz=timezone.utc).strftime("%Y-%m-%d %H:%M UTC")


if __name__ == "__main__":
    raise SystemExit(main())
