use std::collections::BTreeSet;

use serde_json::{Map, Value};
use text::testing::document;

use crate::generated::catalog::CATALOG;
use crate::template::{Filled, Piece, fill};
use crate::{Severity, contract_rules, envelope, messages};

fn fixture(file: &str) -> Value {
    document(env!("CARGO_MANIFEST_DIR"), file)
}

// A fixture string fills its placeholder as it is, and a fixture int as its decimal text.
fn filled(name: &str, value: &Value) -> Filled {
    match value {
        Value::String(text) => Filled::new(text.clone(), false),
        Value::Number(number) => Filled::new(number.to_string(), number.as_u64() == Some(1)),
        other => panic!("{name}: the fixture gives {other}"),
    }
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

    fn filled(&self, name: &str) -> Filled {
        filled(name, &self.arguments[name])
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
        let values: Vec<Filled> = names.iter().map(|name| case.filled(name)).collect();
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
    assert_eq!(cataloged.len(), 243);
    assert_eq!(fixed, cataloged);
}

// The positions of the counts that the template's choices read.
fn chosen(pieces: &[Piece]) -> BTreeSet<usize> {
    let positions = pieces.iter().filter_map(|piece| match piece {
        Piece::Choice(position, _, _) => Some(*position),
        _ => None,
    });
    positions.collect()
}

// The template with each chosen count set to `count` and every other value from the fixture.
fn rendered_at(case: &Value, names: &[&str], pieces: &[Piece], count: usize) -> String {
    let chosen = chosen(pieces);
    let values: Vec<Filled> = (names.iter().enumerate())
        .map(|(position, name)| {
            if chosen.contains(&position) {
                Filled::new(count.to_string(), count == 1)
            } else {
                filled(name, &case["arguments"][*name])
            }
        })
        .collect();
    fill(pieces, &values)
}

#[test]
fn each_choice_takes_the_form_of_its_count_at_one_and_at_two() {
    let fixture = fixture("counts");
    let mut covered: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut mismatches = Vec::new();
    for (file, key, names, pieces) in CATALOG {
        if chosen(pieces).is_empty() {
            continue;
        }
        covered.insert((file, key));
        let case = &fixture[file][key];
        for (count, form) in [(1, "one"), (2, "many")] {
            let rendered = rendered_at(case, names, pieces, count);
            if case[form].as_str() != Some(rendered.as_str()) {
                mismatches.push(format!("{file}.{key} at {count}: {rendered:?}"));
            }
        }
    }
    let fixed: BTreeSet<(&str, &str)> = (fixture.as_object().unwrap().iter())
        .flat_map(|(file, rows)| {
            let keys = rows.as_object().unwrap().keys();
            keys.map(move |key| (file.as_str(), key.as_str()))
        })
        .collect();
    assert_eq!(covered, fixed);
    assert_eq!(covered.len(), 80);
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn a_count_of_one_takes_the_singular_form_in_a_typed_render() {
    assert_eq!(messages::GROUPED_IDS.render(1), "(1 id)");
    assert_eq!(messages::GROUPED_IDS.render(0), "(0 ids)");
    let values = ("src/app/svc.py", 9000, 9050, 1);
    assert_eq!(
        messages::SLICE_RANGE_BEYOND_FILE.render(values),
        "unsatisfiable: line range 9000-9050 is beyond src/app/svc.py (1 line)"
    );
}

// The head of one usage file ends with a colon and takes no order row. A longer head ends a
// sentence, and an order row follows it.
#[test]
fn the_usage_head_of_one_file_ends_with_a_colon_and_of_more_files_with_a_full_stop() {
    assert_eq!(
        messages::FIND_USAGE_HEAD.render(("where", 1)),
        "usage: 1 file spells 'where' and does not declare it:"
    );
    assert_eq!(
        messages::FIND_USAGE_HEAD.render(("where", 47)),
        "usage: 47 files spell 'where' and do not declare it."
    );
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
