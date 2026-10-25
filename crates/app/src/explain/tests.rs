use super::{ExplainRequest, Explanation, explain};
use crate::IntArg;
use crate::endpoints::scanned::{LINKED, Scanned};

fn explained(files: &[(&str, &str)], target: &str) -> Explanation {
    explained_at(files, target, 20)
}

fn explained_at(files: &[(&str, &str)], target: &str, limit: usize) -> Explanation {
    let scanned = Scanned::new(files);
    let request = ExplainRequest {
        target: target.to_owned(),
        limit: IntArg::from_usize(limit),
    };
    let outcome = explain(&scanned.services, &scanned.reading(), &request);
    outcome.value.unwrap_or_else(|error| panic!("{error}"))
}

// Two classes that each define `as_sql`, so the bare name has two definitions.
const TWO_METHODS: [(&str, &str); 1] = [(
    "a.py",
    "class Value:\n    def as_sql(self):\n        return 1\n\n\nclass Other:\n    def \
     as_sql(self):\n        return 2\n",
)];

#[test]
fn a_qualified_name_explains_its_one_definition_with_no_ties() {
    let answer = explained(&TWO_METHODS, "Value.as_sql");
    let card = answer.card.unwrap_or_else(|| panic!("a card"));
    assert_eq!(card.stable_id, "py:a.py::Value.as_sql");
    assert_eq!(answer.tied, 1);
    assert!(answer.alternatives.rows().is_empty());
    assert_eq!(answer.alternatives.total(), 0);
}

#[test]
fn the_limit_cuts_the_alternatives_and_keeps_their_count() {
    let files =
        ["a", "b", "c", "d", "e"].map(|stem| (format!("{stem}.py"), "def run():\n    return 1\n"));
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), *text))
        .collect();
    let answer = explained_at(&files, "run", 2);
    assert_eq!(answer.tied, 5);
    assert_eq!(answer.alternatives.rows(), ["py:b.py::run", "py:c.py::run"]);
    assert_eq!(answer.alternatives.total(), 4);
    assert_eq!(answer.alternatives.omitted(), 2);
}

#[test]
fn a_data_key_id_explains_its_card_and_says_a_key_has_no_fan_in() {
    let files = [("data.json", "{\n  \"receipt_header\": \"x\"\n}\n")];
    let answer = explained(&files, "json:data.json::receipt_header");
    let card = answer.card.unwrap_or_else(|| panic!("a card"));
    assert_eq!(card.stable_id, "json:data.json::receipt_header");
    assert_eq!(
        answer.message,
        "json:data.json::receipt_header is a data key, and a data key has no fan-in: Grep finds \
         the spellings of receipt_header."
    );
    assert!(answer.data_key);
    assert!(answer.fan_in.is_empty() && answer.fan_out.is_empty());
}

#[test]
fn nothing_matching_is_a_message_and_no_card() {
    let answer = explained(&LINKED, "zz_nothing");
    assert!(answer.card.is_none());
    assert_eq!(answer.message, "no symbol matches zz_nothing");
    assert_eq!(answer.imports_out.total(), 0);
}

#[test]
fn the_card_carries_its_tiered_fan_in_and_the_imports_of_its_file() {
    let answer = explained(&LINKED, "format_money");
    let card = answer.card.unwrap_or_else(|| panic!("a card"));
    assert_eq!(card.stable_id, "py:pkg/money.py::format_money");
    assert!(card.body.is_empty());
    let fan_in: Vec<(&str, usize, &str)> = (answer.fan_in.iter())
        .flat_map(|group| {
            group
                .rows
                .iter()
                .map(move |row| (group.tier, group.total, row.relation))
        })
        .collect();
    assert_eq!(fan_in, [("imported", 1, "referenced by")]);
    assert!(answer.imports_out.rows().is_empty());
    let importers: Vec<&str> = (answer.imports_in.rows().iter())
        .map(|row| row.path.as_str())
        .collect();
    assert_eq!(importers, ["pkg/report.py"]);
}

#[test]
fn every_other_definition_is_an_alternative_and_the_exact_band_counts_the_ties() {
    let files = [
        ("a.py", "def run():\n    return 1\n"),
        ("b.py", "def run():\n    return 2\n"),
    ];
    let answer = explained(&files, "run");
    assert_eq!(answer.tied, 2);
    assert_eq!(answer.alternatives.rows(), ["py:b.py::run"]);
}
