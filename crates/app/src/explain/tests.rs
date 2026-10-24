use super::{ExplainRequest, Explanation, explain};
use crate::IntArg;
use crate::endpoints::scanned::{LINKED, Scanned};

fn explained(files: &[(&str, &str)], target: &str) -> Explanation {
    let scanned = Scanned::new(files);
    let request = ExplainRequest {
        target: target.to_owned(),
        limit: IntArg::from_usize(20),
    };
    let outcome = explain(&scanned.services, &scanned.reading(), &request);
    outcome.value.unwrap_or_else(|error| panic!("{error}"))
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
    assert_eq!(answer.alternatives, ["py:b.py::run"]);
}
