use serde::Serialize;
use serde_json::Value;

use super::rows::{CapabilityRow, ReceiptRow, cases, report};
use crate::envelope::{wrap, wrap_json};

#[test]
fn each_body_wraps_as_the_fixture_says() {
    let all: Vec<(ReceiptRow, String, usize, String)> = cases("envelope", "text");
    for (row, body, ceiling, expected) in all {
        let config = row.config();
        let wrapped = wrap(&row.receipt(&config), &body, ceiling);
        assert_eq!(wrapped, expected, "{body:?} at {ceiling}");
    }
}

#[test]
fn each_lint_document_wraps_as_the_fixture_says() {
    let all: Vec<(ReceiptRow, Vec<Value>, usize, String)> = cases("envelope", "json");
    for (row, rows, ceiling, expected) in all {
        let config = row.config();
        let wrapped = wrap_json(&row.receipt(&config), &report(&rows), ceiling);
        let wrapped = wrapped.unwrap_or_else(|error| panic!("{rows:?}: {error}"));
        assert_eq!(wrapped, expected, "{rows:?} at {ceiling}");
    }
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
        for reserved in ["receipt", "notice", "truncated"] {
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
