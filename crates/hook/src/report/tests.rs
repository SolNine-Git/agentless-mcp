use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use super::*;
use crate::report::classify::{Classified, classify};
use crate::report::csv::{Cell, DictReader, excel_row};
use crate::report::labels::LabelCount;
use crate::report::log::read_log;
use crate::report::printed::{verdict, when};
use crate::report::random::{Twister, set_size};
use crate::report::transcript::read_transcripts;
use crate::testing::{MANIFEST_DIR, Sandbox, latin};

fn section(file: &str, name: &str) -> Vec<Value> {
    io::testing::cases(MANIFEST_DIR, &format!("report/{file}"), name)
}

fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not a text: {value}"))
}

fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| panic!("not a count: {value}"))
}

fn count(value: &Value) -> usize {
    usize::try_from(number(value)).unwrap_or_else(|error| panic!("{value}: {error}"))
}

fn read(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

// The class of each finding in a log, as the fixture writes them.
fn classified(log: &str) -> Option<Vec<Value>> {
    let records = read_log(&pure(log)).ok()?;
    let transcripts = read_transcripts(&records).ok()?;
    let found = classify(&records, &transcripts);
    let row =
        |item: &Classified<'_>| json!([crate::text::lossy(&item.finding_id), item.class.name()]);
    Some(found.iter().map(row).collect())
}

// A crash of the reference is one line after the prefix, exit 1, and nothing on stdout.
fn expected(step: &[Value]) -> (String, String, u64) {
    match step[5].as_str() {
        Some(crash) => (
            String::new(),
            format!("{}{PREFIX}{crash}\n", text(&step[3])),
            1,
        ),
        None => (
            text(&step[2]).to_owned(),
            text(&step[3]).to_owned(),
            number(&step[4]),
        ),
    }
}

fn run_step(sandbox: &Sandbox, step: &[Value], name: &str) {
    let request = &step[1];
    let place = |key: &str| request[key].as_str().map(|value| sandbox.placed(value));
    let log = sandbox.placed(text(&request["log"]));
    let (sample_out, labels) = (place("sample_out"), place("labels"));
    let seed = text(&request["seed"]);
    let output = gate_report(&log, sample_out.as_deref(), labels.as_deref(), seed);
    let (stdout, stderr, code) = expected(step);
    assert_eq!(
        sandbox.placeholders(&output.stdout),
        stdout,
        "{name}: stdout"
    );
    assert_eq!(
        sandbox.placeholders(&output.stderr),
        stderr,
        "{name}: stderr"
    );
    assert_eq!(u64::from(output.code), code, "{name}: exit code");
}

// Runs each step, then compares the files the runs made, the files they left, and the classes.
fn check_case(case: &Value) {
    let name = text(&case["name"]);
    let sandbox = Sandbox::new();
    let mut put: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut log = None;
    for step in case["steps"].as_array().into_iter().flatten() {
        let step = step.as_array().map_or(&[][..], Vec::as_slice);
        match text(&step[0]) {
            "put" => {
                let bytes = latin(&sandbox.placed(text(&step[2])));
                sandbox.put(text(&step[1]), &bytes);
                put.insert(text(&step[1]).to_owned(), bytes);
            }
            "mkdir" => drop(sandbox.mkdir(text(&step[1]))),
            _ => {
                log = Some(sandbox.placed(text(&step[1]["log"])));
                run_step(&sandbox, step, name);
            }
        }
    }
    let made: BTreeMap<&str, &str> = case["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|file| (text(&file[0]), text(&file[1])))
        .collect();
    for (rel, bytes) in &made {
        let found = read(&sandbox.path().join(rel));
        assert_eq!(found, latin(&sandbox.placed(bytes)), "{name}: {rel}");
    }
    for (rel, bytes) in put
        .iter()
        .filter(|(rel, _)| !made.contains_key(rel.as_str()))
    {
        assert_eq!(
            &read(&sandbox.path().join(rel)),
            bytes,
            "{name}: {rel} changed"
        );
    }
    if let (Some(expected), Some(log)) = (case["classified"].as_array(), log) {
        assert_eq!(classified(&log).as_ref(), Some(expected), "{name}: classes");
    }
}

#[test]
fn the_runs_equal_the_reference() {
    let cases = section("runs", "runs");
    assert!(cases.len() > 90, "{} cases", cases.len());
    cases.iter().for_each(check_case);
}

#[test]
fn the_large_runs_equal_the_reference() {
    section("large", "runs").iter().for_each(check_case);
}

#[test]
fn a_log_that_the_reference_hook_wrote_reads_as_the_reference_reads_it() {
    section("hooklog", "runs").iter().for_each(check_case);
}

#[test]
fn getrandbits_cases() {
    for case in section("units", "bits") {
        let mut twister = Twister::from_text(text(&case[0]));
        for draw in case[1].as_array().into_iter().flatten() {
            let k = u32::try_from(number(&draw[0])).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(twister.bits(k), number(&draw[1]), "{case}");
        }
    }
}

#[test]
fn randbelow_cases() {
    let mut twister = Twister::from_text("0:clone_delta");
    for case in section("units", "below") {
        let n = number(&case[0]);
        for draw in case[1].as_array().into_iter().flatten() {
            assert_eq!(twister.below(n), number(draw), "n {n}");
        }
    }
}

#[test]
fn sample_cases_reach_both_branches() {
    let mut branches = (0, 0);
    for case in section("units", "sample") {
        let (n, k) = (count(&case[1]), count(&case[2]));
        let expected: Vec<usize> = case[3]
            .as_array()
            .into_iter()
            .flatten()
            .map(count)
            .collect();
        assert_eq!(
            Twister::from_text(text(&case[0])).sample(n, k),
            expected,
            "{case}"
        );
        if n <= set_size(k) {
            branches.0 += 1;
        } else {
            branches.1 += 1;
        }
    }
    assert!(branches.0 > 0 && branches.1 > 0, "{branches:?}");
}

#[test]
fn when_cases() {
    for case in section("units", "when") {
        let expected = match case[1].as_str() {
            Some(shown) => Ok(shown.to_owned()),
            None => Err(text(&case[2]).to_owned()),
        };
        assert_eq!(
            when(::text::testing::float(text(&case[0]))),
            expected,
            "{case}"
        );
    }
}

fn cell_value(cell: &Cell) -> Value {
    match cell {
        Cell::Text(value) => json!(value),
        Cell::Missing => Value::Null,
        Cell::Rest(values) => json!(values),
    }
}

// What the reader reads from bytes: the header, each row with its line count, and the error.
fn read_back(bytes: &[u8]) -> Value {
    let mut reader = DictReader::new(bytes);
    let names = match reader.fieldnames() {
        Ok(names) => names.map(<[String]>::to_vec),
        Err(error) => return json!({"fieldnames": null, "rows": [], "error": error}),
    };
    let mut rows = Vec::new();
    while let Some(row) = reader.next_row() {
        let row = match row {
            Ok(row) => row,
            Err(error) => return json!({"fieldnames": names, "rows": rows, "error": error}),
        };
        let pairs: Vec<Value> = row
            .iter()
            .map(|(key, cell)| json!([key, cell_value(cell)]))
            .collect();
        rows.push(json!([reader.line_num(), pairs]));
    }
    json!({"fieldnames": names, "rows": rows, "error": null})
}

#[test]
fn csv_read_cases() {
    for (index, case) in section("units", "csv_read").iter().enumerate() {
        assert_eq!(read_back(&latin(text(&case[0]))), case[1], "case {index}");
    }
}

#[test]
fn csv_write_cases() {
    for case in section("units", "csv_write") {
        let fields: Vec<String> = case[0]
            .as_array()
            .into_iter()
            .flatten()
            .map(|field| text(field).to_owned())
            .collect();
        assert_eq!(
            excel_row(&fields).into_bytes(),
            latin(text(&case[1])),
            "{case}"
        );
    }
}

#[test]
fn verdict_cases() {
    for case in section("units", "verdict") {
        let labels = case[1].as_array().map(|counts| LabelCount {
            correct: count(&counts[0]),
            wrong: count(&counts[1]),
            unlabeled: count(&counts[2]),
        });
        assert_eq!(verdict(count(&case[0]), labels), text(&case[2]), "{case}");
    }
}

fn pair(check: &'static str, detail: checks::Detail) -> crate::record::Logged {
    let finding = checks::Finding {
        check,
        severity: checks::Severity::Warning,
        message: "m".to_owned(),
        path: "pkg/app.py".to_owned(),
        line: 3,
        evidence: "e".to_owned(),
    };
    let row = checks::PairFinding {
        finding,
        detail: Some(detail),
    };
    crate::record::Logged::of(&row, io::GateMode::Advisory)
}

fn full_state() -> checks::Observed {
    let key = kernel::Key::new([4; 32]);
    let (path, unit) = ("pkg/app.py".to_owned(), "big".to_owned());
    checks::Observed {
        units_over_threshold: Some(vec![checks::UnitState {
            path: path.clone(),
            unit: unit.clone(),
            line: 1,
            cc: 12,
            sloc: 20,
        }]),
        units_changed: Some(vec![checks::UnitChange {
            path: path.clone(),
            unit: unit.clone(),
            line: 1,
            via: checks::Match::Collision,
            cc_old: None,
            cc_new: 12,
            sloc_old: Some(2),
            sloc_new: 20,
        }]),
        contract_keys: Some(vec![checks::HitState {
            path: path.clone(),
            rule: "r".to_owned(),
            unit,
            capture: "c".to_owned(),
        }]),
        clone_instances: Some(vec![checks::CloneState {
            path,
            key,
            line: 1,
            end_line: 6,
        }]),
    }
}

fn written_record() -> crate::record::Record {
    let key = kernel::Key::new([4; 32]);
    let numbers = checks::UnitDelta {
        unit: "big".to_owned(),
        via: checks::Match::Body,
        cc_old: Some(11),
        cc_new: 13,
        sloc_old: None,
        sloc_new: 22,
        threshold: 10,
    };
    let drift = checks::DriftKey {
        rule: "r".to_owned(),
        unit: "big".to_owned(),
        capture: "c".to_owned(),
    };
    let site = checks::CloneSite {
        path: "pkg/b.py".to_owned(),
        line: 1,
        end_line: 6,
    };
    let copy = checks::CloneCopy {
        key,
        line: 20,
        end_line: 25,
        sites: vec![site],
        sites_total: 1,
    };
    let mut record = crate::record::Record::blank(1_791_600_000.0);
    record.identify(crate::record::Identity {
        session_id: Some("s1".to_owned()),
        transcript_path: Some("/nowhere/t1.jsonl".to_owned()),
        tool_use_id: Some("toolu_1".to_owned()),
        tool_name: Some("Edit".to_owned()),
    });
    record.path = Some("/repo/pkg/app.py".to_owned());
    record.settle(crate::record::Settled {
        outcome: crate::record::Outcome::Advisory,
        reason: None,
        gaps: Vec::new(),
        off_by_config: Vec::new(),
        findings: vec![
            pair(checks::COMPLEXITY_DELTA, checks::Detail::Unit(numbers)),
            pair(checks::CONTRACT_DRIFT, checks::Detail::Drift(drift)),
            pair(checks::CLONE_DELTA, checks::Detail::Copy(copy)),
        ],
        observed: Some(full_state()),
    });
    record
}

// The writer's keys are the reader's: a record of every finding kind and every observed list,
// as the gate writes it, reads back whole.
#[test]
fn the_report_reads_every_field_that_the_gate_writes() {
    let sandbox = Sandbox::new();
    let line = json::to_text(&written_record(), json::JsonForm::Compact)
        .unwrap_or_else(|error| panic!("{error}"));
    let log = sandbox.put("gate.jsonl", format!("{line}\n"));
    let records = read_log(&log.display().to_string()).unwrap_or_else(|error| panic!("{error}"));
    let [record] = records.as_slice() else {
        panic!("{} records", records.len());
    };
    let checks: Vec<String> = record
        .findings
        .iter()
        .map(|finding| crate::text::lossy(&finding.check))
        .collect();
    assert_eq!(
        checks,
        ["complexity_delta", "contract_drift", "clone_delta"]
    );
    assert!(
        record
            .findings
            .iter()
            .all(|finding| finding.tracked.is_some())
    );
    assert_eq!(record.findings[0].via, Some(checks::Match::Body));
    let state = record
        .observed
        .as_ref()
        .unwrap_or_else(|| panic!("no observed state"));
    let held = [
        state.units_over_threshold.as_ref().map(Vec::len),
        state.contract_keys.as_ref().map(Vec::len),
        state.clone_instances.as_ref().map(Vec::len),
    ];
    assert_eq!(held, [Some(1); 3]);
}

#[test]
fn a_sample_file_under_a_missing_directory_is_one_traceback_line() {
    let sandbox = Sandbox::new();
    sandbox.put("gate.jsonl", b"");
    let shown = |relative: &str| sandbox.path().join(relative).display().to_string();
    let output = gate_report(&shown("gate.jsonl"), Some(&shown("gone/s.csv")), None, "0");
    let expected = format!(
        "{PREFIX}FileNotFoundError: [Errno 2] No such file or directory: '{}'\n",
        shown("gone/s.csv")
    );
    assert_eq!(
        (output.code, output.stderr, output.stdout),
        (1, expected, String::new())
    );
}
