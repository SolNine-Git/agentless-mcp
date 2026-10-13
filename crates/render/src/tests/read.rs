use std::collections::BTreeSet;

use app::{Chars4, TokenCounter};
use serde::Serialize;
use serde_json::Value;

use super::rows::{ReceiptRow, cases};
use crate::count::Ceiling;
use crate::envelope::{Listing, wrap, wrap_json};
use crate::receipt_lines;

const FIXTURE: &str = "read/envelope";

// Counts the distinct code points of a text. It is not additive, so it shows each place that
// counts: the header, the warnings block, the body prefixes and each JSON probe.
struct Distinct;

impl TokenCounter for Distinct {
    fn count(&self, text: &str) -> usize {
        text.chars().collect::<BTreeSet<char>>().len()
    }
}

// A payload held as a JSON value, with its list at a dotted key, as the reference names it.
#[derive(Clone)]
struct Dotted {
    document: Value,
    key: String,
}

impl Dotted {
    fn list(&self) -> &Vec<Value> {
        let found = self
            .key
            .split('.')
            .try_fold(&self.document, |value, step| value.get(step));
        found
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("no list at {}", self.key))
    }
}

impl Serialize for Dotted {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.document.serialize(serializer)
    }
}

impl Listing for Dotted {
    fn items(&self) -> usize {
        self.list().len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut copy = self.clone();
        let list = copy
            .key
            .split('.')
            .try_fold(&mut copy.document, |value, step| value.get_mut(step))
            .and_then(Value::as_array_mut)
            .unwrap_or_else(|| panic!("no list at {}", self.key));
        list.truncate(kept);
        copy
    }
}

fn wrapped_json(row: &ReceiptRow, payload: Value, key: String, ceiling: &Ceiling<'_>) -> String {
    let config = row.config();
    let listing = Dotted {
        document: payload,
        key,
    };
    let wrapped = wrap_json(&row.receipt(&config), &listing, ceiling);
    wrapped.unwrap_or_else(|error| panic!("{error}"))
}

// A list at a dotted key trims as the reference trims "bodies.symbols".
#[test]
fn a_nested_list_trims_as_the_reference_says() {
    let all: Vec<(ReceiptRow, Value, String, usize, String)> = cases(FIXTURE, "nested");
    assert!(!all.is_empty());
    for (row, payload, key, tokens, expected) in all {
        let ceiling = Ceiling {
            counter: &Chars4,
            tokens,
        };
        let wrapped = wrapped_json(&row, payload, key.clone(), &ceiling);
        assert_eq!(wrapped, expected, "{key} at {tokens}");
    }
}

#[test]
fn each_stderr_receipt_matches_the_reference_lines() {
    let all: Vec<(ReceiptRow, String, Vec<String>)> = cases(FIXTURE, "receipt_lines");
    assert!(!all.is_empty());
    for (row, summary, expected) in all {
        let config = row.config();
        let lines = receipt_lines(&row.receipt(&config), &summary);
        assert_eq!(lines, expected, "{summary:?}");
    }
}

#[test]
fn every_text_count_goes_through_the_counter() {
    type Case = (ReceiptRow, String, usize, Option<Value>, String);
    let all: Vec<Case> = cases(FIXTURE, "distinct_text");
    assert!(!all.is_empty());
    for (row, body, tokens, truncation, expected) in all {
        assert_eq!(truncation, None);
        let config = row.config();
        let ceiling = Ceiling {
            counter: &Distinct,
            tokens,
        };
        assert_eq!(
            wrap(&row.receipt(&config), &body, &ceiling),
            expected,
            "at {tokens}"
        );
    }
}

#[test]
fn every_json_count_goes_through_the_counter() {
    let all: Vec<(ReceiptRow, Value, String, usize, String)> = cases(FIXTURE, "distinct_json");
    assert!(!all.is_empty());
    for (row, payload, key, tokens, expected) in all {
        let ceiling = Ceiling {
            counter: &Distinct,
            tokens,
        };
        let wrapped = wrapped_json(&row, payload, key.clone(), &ceiling);
        assert_eq!(wrapped, expected, "{key} at {tokens}");
    }
}
