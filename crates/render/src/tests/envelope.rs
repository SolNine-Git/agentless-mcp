use app::Chars4;
use serde::Serialize;
use serde_json::Value;

use super::rows::{CapabilityRow, ReceiptRow, cases, report};
use crate::count::Ceiling;
use crate::envelope::{wrap, wrap_json};

fn chars4(tokens: usize) -> Ceiling<'static> {
    Ceiling {
        counter: &Chars4,
        tokens,
    }
}

#[test]
fn each_body_wraps_as_the_fixture_says() {
    let all: Vec<(ReceiptRow, String, usize, String)> = cases("envelope", "text");
    for (row, body, ceiling, expected) in all {
        let config = row.config();
        let wrapped = wrap(&row.receipt(&config), &body, &chars4(ceiling), None);
        assert_eq!(wrapped, expected, "{body:?} at {ceiling}");
    }
}

#[test]
fn each_lint_document_wraps_as_the_fixture_says() {
    let all: Vec<(ReceiptRow, Vec<Value>, usize, String)> = cases("envelope", "json");
    for (row, rows, ceiling, expected) in all {
        let config = row.config();
        let wrapped = wrap_json(&row.receipt(&config), &report(&rows), &chars4(ceiling));
        let wrapped = wrapped.unwrap_or_else(|error| panic!("{rows:?}: {error}"));
        assert_eq!(wrapped, expected, "{rows:?} at {ceiling}");
    }
}

#[test]
fn a_whole_payload_over_the_ceiling_is_over_ceiling_and_a_cut_one_is_truncated() {
    let all: Vec<(ReceiptRow, Vec<Value>, usize, String)> = cases("envelope", "json");
    let largest = all.iter().max_by_key(|(_, rows, _, _)| rows.len());
    let (row, rows, _, _) = largest.unwrap_or_else(|| panic!("no lint case"));
    assert!(rows.len() > 1);
    let config = row.config();
    let keys = |rows: &[Value]| {
        let wrapped = wrap_json(&row.receipt(&config), &report(rows), &chars4(1));
        let text = wrapped.unwrap_or_else(|error| panic!("{error}"));
        let document: serde_json::Map<String, Value> =
            serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}"));
        ["truncated", "over_ceiling"].map(|key| document.contains_key(key))
    };
    assert_eq!(keys(&rows[..1]), [false, true]);
    assert_eq!(keys(rows), [true, false]);
}

// The envelope writes these keys itself, so no payload may hold one at its top level.
#[test]
fn no_payload_holds_a_key_of_the_envelope() {
    let lint: Vec<(ReceiptRow, Vec<Value>, usize, String)> = cases("envelope", "json");
    let capabilities: Vec<(CapabilityRow, String)> = cases("capabilities", "text");
    let rows = lint
        .iter()
        .map(|(_, rows, _, _)| rows)
        .max_by_key(|rows| rows.len());
    let rows = rows.unwrap_or_else(|| panic!("no lint case"));
    let (row, _) = &capabilities[0];
    let config = row.config();
    let payloads = [
        top_level_keys(&report(rows)),
        top_level_keys(&row.capabilities(&config)),
    ];
    for keys in payloads {
        assert!(!keys.is_empty());
        for reserved in ["receipt", "notice", "truncated", "over_ceiling"] {
            assert!(
                !keys.iter().any(|key| key == reserved),
                "{reserved} in {keys:?}"
            );
        }
    }
}

fn top_level_keys(payload: &impl Serialize) -> Vec<String> {
    let text = json::to_text(payload, json::JsonForm::Compact);
    let text = text.unwrap_or_else(|error| panic!("{error}"));
    let document: serde_json::Map<String, Value> =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}"));
    document.keys().cloned().collect()
}
