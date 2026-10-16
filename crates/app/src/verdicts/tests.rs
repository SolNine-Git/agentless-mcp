use patch::{Regression, VoteCandidate};
use serde_json::{Value, json};

use super::{LoadedVerdicts, load_verdicts};
use crate::testing::MANIFEST_DIR;

// A candidate as the reference writes it: measured and passed are the two halves of Regression.
fn candidate_row(candidate: &VoteCandidate) -> Value {
    json!([
        candidate.id,
        candidate.index,
        candidate.applied,
        candidate.regression != Regression::NotMeasured,
        candidate.equivalence_key,
        candidate.regression == Regression::Passed,
        candidate.reproduction_passed,
    ])
}

fn loaded_row(loaded: &LoadedVerdicts) -> Value {
    let candidates: Vec<Value> = loaded.candidates.iter().map(candidate_row).collect();
    json!({
        "baseline": loaded.baseline.as_str(),
        "repro_valid": loaded.repro_valid,
        "test_cmd": loaded.test_cmd,
        "repro_cmd": loaded.repro_cmd,
        "candidates": candidates,
    })
}

fn texts() -> Vec<(String, Value)> {
    io::testing::cases(MANIFEST_DIR, "validate/verdicts", "texts")
}

#[test]
fn each_verdicts_text_reads_to_the_reference_document_or_refusal() {
    let cases = texts();
    assert!(cases.len() >= 50, "{} cases", cases.len());
    for (text, expected) in &cases {
        let found = match load_verdicts(text) {
            Ok(loaded) => loaded_row(&loaded),
            Err(error) => json!(error.to_string()),
        };
        let wanted = expected.get("loaded").or_else(|| expected.get("error"));
        assert_eq!(Some(&found), wanted, "{text:?}");
    }
}

// A run record with `test_cmd`, a JSON text that can escape a lone surrogate.
fn header(test_cmd: &str) -> String {
    let head = r#"{"record": "run", "baseline": "ok", "repro_valid": true"#;
    format!(r#"{head}, "test_cmd": {test_cmd}, "repro_cmd": null}}"#)
}

// A candidate record with `fields` before the fields that every candidate here shares.
fn candidate(fields: &str) -> String {
    let tail = r#""apply": {"status": "ok"}, "regression": "passed", "reproduction": null"#;
    format!(r#"{{"record": "candidate", {fields}, {tail}}}"#)
}

fn document(candidate: &str) -> String {
    format!("{}\n{candidate}\n", header(r#""t""#))
}

#[test]
fn an_index_outside_64_bits_is_refused_with_its_line() {
    let candidate = candidate(r#""id": "c", "index": 9223372036854775808"#);

    let refused = load_verdicts(&document(&candidate)).expect_err("a refusal");

    assert_eq!(
        refused.to_string(),
        "line 2: 'index' is outside the 64-bit integer range"
    );
}

#[test]
fn a_lone_surrogate_in_a_text_field_is_refused_with_its_line() {
    let fields = [
        ("id", r#""id": "\ud800", "equivalence_key": null"#),
        (
            "equivalence_key",
            r#""id": "c", "equivalence_key": "\udfff""#,
        ),
    ];
    for (name, field) in fields {
        let candidate = candidate(&format!(r#"{field}, "index": 0"#));

        let refused = load_verdicts(&document(&candidate)).expect_err("a refusal");

        assert_eq!(
            refused.to_string(),
            format!("line 2: '{name}' has a lone surrogate")
        );
    }
}

#[test]
fn a_lone_surrogate_in_a_command_is_refused_on_the_first_line() {
    let text = header(r#""\udc80""#);

    let refused = load_verdicts(&text).expect_err("a refusal");

    assert_eq!(
        refused.to_string(),
        "line 1: 'test_cmd' has a lone surrogate"
    );
}
