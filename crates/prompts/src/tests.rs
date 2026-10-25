use std::collections::BTreeSet;

use serde_json::{Map, Value};
use text::testing::document;

use crate::generated::catalog::CATALOG;
use crate::template::fill;
use crate::{Severity, contract_rules, envelope, messages};

fn fixture(file: &str) -> Value {
    document(env!("CARGO_MANIFEST_DIR"), file)
}

struct Case<'f> {
    arguments: &'f Map<String, Value>,
    text: &'f str,
}

impl<'f> Case<'f> {
    fn of(fixture: &'f Value, file: &str, key: &str) -> Self {
        let case = &fixture[file][key];
        Self {
            arguments: case["arguments"]
                .as_object()
                .unwrap_or_else(|| panic!("{file}.{key}: the fixture has no arguments")),
            text: case["text"]
                .as_str()
                .unwrap_or_else(|| panic!("{file}.{key}: the fixture has no text")),
        }
    }

    fn text(&self, name: &str) -> &'f str {
        self.arguments[name]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: the fixture gives no text"))
    }

    fn count(&self, name: &str) -> usize {
        self.arguments[name]
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or_else(|| panic!("{name}: the fixture gives no count"))
    }

    // A fixture string fills its placeholder as it is, and a fixture int as its decimal text.
    fn formatted(&self, name: &str) -> String {
        match &self.arguments[name] {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            other => panic!("{name}: the fixture gives {other}"),
        }
    }
}

#[test]
fn every_template_renders_the_text_of_the_fixture() {
    let fixture = fixture("render");
    let mut mismatches = Vec::new();
    for (file, key, names, pieces) in CATALOG {
        let case = Case::of(&fixture, file, key);
        let given: BTreeSet<&str> = case.arguments.keys().map(String::as_str).collect();
        let wanted: BTreeSet<&str> = names.iter().copied().collect();
        if given != wanted {
            mismatches.push(format!(
                "{file}.{key}: the fixture gives {given:?}, the template names {wanted:?}"
            ));
            continue;
        }
        let values: Vec<String> = names.iter().map(|name| case.formatted(name)).collect();
        let rendered = fill(pieces, &values);
        if rendered != case.text {
            mismatches.push(format!("{file}.{key}: {rendered:?} is not {:?}", case.text));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn the_fixture_and_the_catalog_hold_the_same_templates() {
    let fixture = fixture("render");
    let files: BTreeSet<&str> = fixture
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(files, BTreeSet::from(["envelope", "messages"]));
    let fixed: BTreeSet<(&str, &str)> = files
        .iter()
        .flat_map(|file| {
            fixture[*file]
                .as_object()
                .unwrap()
                .keys()
                .map(|key| (*file, key.as_str()))
        })
        .collect();
    let cataloged: BTreeSet<(&str, &str)> = CATALOG
        .iter()
        .map(|(file, key, _, _)| (*file, *key))
        .collect();
    assert_eq!(cataloged.len(), 156);
    assert_eq!(fixed, cataloged);
}

#[test]
fn a_typed_render_puts_each_value_in_its_place() {
    let fixture = fixture("render");
    let case = Case::of(&fixture, "messages", "find_no_matches");
    assert_eq!(
        messages::FIND_NO_MATCHES.render(case.text("query")),
        case.text
    );
    let case = Case::of(&fixture, "messages", "grouped_ids");
    assert_eq!(messages::GROUPED_IDS.render(case.count("count")), case.text);
    let case = Case::of(&fixture, "messages", "pair_edits_unapplied");
    let values = (case.text("path"), case.count("failed"));
    assert_eq!(messages::PAIR_EDITS_UNAPPLIED.render(values), case.text);
    let case = Case::of(&fixture, "messages", "slice_range_not_a_range");
    let values = (case.text("path"), case.count("start"), case.count("end"));
    assert_eq!(messages::SLICE_RANGE_NOT_A_RANGE.render(values), case.text);
    let case = Case::of(&fixture, "messages", "slice_range_beyond_file");
    let values = (
        case.text("path"),
        case.count("start"),
        case.count("end"),
        case.count("total"),
    );
    assert_eq!(messages::SLICE_RANGE_BEYOND_FILE.render(values), case.text);
    let case = Case::of(&fixture, "envelope", "receipt_fields");
    let values = (case.text("head"), case.text("dirty"), case.text("cache"));
    assert_eq!(envelope::RECEIPT_FIELDS.render(values), case.text);
    let case = Case::of(&fixture, "envelope", "receipt_repo");
    assert_eq!(envelope::RECEIPT_REPO.render(case.text("root")), case.text);
}

#[test]
fn a_typed_render_of_five_and_six_values_keeps_their_order() {
    let fixture = fixture("render");
    let case = Case::of(&fixture, "messages", "complexity_evidence");
    let numbers =
        ["old_cc", "new_cc", "old_sloc", "new_sloc", "threshold"].map(|name| case.count(name));
    let [old_cc, new_cc, old_sloc, new_sloc, threshold] = numbers;
    let values = (old_cc, new_cc, old_sloc, new_sloc, threshold);
    assert_eq!(messages::COMPLEXITY_EVIDENCE.render(values), case.text);
    let case = Case::of(&fixture, "messages", "complexity_grew");
    let values = (
        case.text("qualname"),
        case.count("old_cc"),
        case.count("new_cc"),
        case.count("old_sloc"),
        case.count("new_sloc"),
        case.count("threshold"),
    );
    assert_eq!(messages::COMPLEXITY_GREW.render(values), case.text);
}

#[test]
fn a_text_with_no_values_is_the_fixture_text() {
    let fixture = fixture("render");
    assert_eq!(
        envelope::RECEIPT_HEADER,
        Case::of(&fixture, "envelope", "receipt_header").text
    );
    assert_eq!(
        messages::SERVER_NO_ROOTS,
        Case::of(&fixture, "messages", "server_no_roots").text
    );
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Warning => "warning",
        Severity::Advisory => "advisory",
    }
}

#[test]
fn the_contract_rules_are_the_fixture_records_in_order() {
    let fixture = fixture("contract_rules");
    let records = fixture.as_array().unwrap();
    let rules = contract_rules();
    assert_eq!(rules.len(), records.len());
    for (rule, record) in rules.iter().zip(records) {
        assert_eq!(record["id"], rule.id);
        assert_eq!(record["language"], rule.language, "{}", rule.id);
        assert_eq!(
            record["severity"],
            severity_name(rule.severity),
            "{}",
            rule.id
        );
        assert_eq!(record["query"], rule.query, "{}", rule.id);
        assert_eq!(record["message"], rule.message, "{}", rule.id);
    }
}

#[test]
fn the_contract_rules_keep_the_rule_file_checks() {
    let rules = contract_rules();
    assert_eq!(rules.len(), 30);
    let ids: BTreeSet<&str> = rules.iter().map(|rule| rule.id).collect();
    assert_eq!(ids.len(), 19);
    let pairs: BTreeSet<(&str, &str)> = rules.iter().map(|rule| (rule.id, rule.language)).collect();
    assert_eq!(
        pairs.len(),
        rules.len(),
        "a rule is defined twice for one language"
    );
    for rule in rules {
        let fields = [rule.id, rule.language, rule.query, rule.message];
        assert!(
            fields.iter().all(|field| !field.trim().is_empty()),
            "{rule:?}"
        );
    }
}
