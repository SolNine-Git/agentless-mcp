use serde::Deserialize;

use super::{CHECKS, FormKind, PairCheck, PairSelection, unknown_check_refusal};
use crate::testing::MANIFEST_DIR;

fn cases<T: serde::de::DeserializeOwned>(section: &str) -> T {
    io::testing::cases(MANIFEST_DIR, "registry", section)
}

#[derive(Deserialize)]
enum Selected {
    #[serde(rename = "chosen")]
    Chosen(Vec<String>),
    #[serde(rename = "refused")]
    Refused(String),
}

fn names(selection: PairSelection) -> Vec<String> {
    selection
        .checks()
        .map(|check| check.name().to_owned())
        .collect()
}

#[test]
fn the_registry_lists_each_check_with_its_input_in_order() {
    let expected: Vec<(String, String)> = cases("checks");
    let found: Vec<(String, String)> = CHECKS
        .iter()
        .map(|check| (check.name.to_owned(), check.input.as_str().to_owned()))
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn the_three_erosion_checks_are_the_pair_checks() {
    let [expected]: [Vec<String>; 1] = cases("pair_checks");
    let found: Vec<&str> = PairCheck::ALL.map(PairCheck::name).to_vec();
    assert_eq!(found, expected);
}

#[test]
fn the_default_reads_only_the_edited_files() {
    let [expected]: [Vec<String>; 1] = cases("edited_file_checks");
    assert_eq!(names(PairSelection::edited_files()), expected);
}

#[test]
fn an_unknown_or_empty_selection_is_refused_with_the_reference_text() {
    let rows: Vec<(Vec<String>, Option<String>)> = cases("unknown_check_refusal");
    for (names, expected) in rows {
        assert_eq!(unknown_check_refusal(&names), expected, "{names:?}");
    }
}

#[test]
fn a_selection_runs_its_pair_checks_in_registry_order_or_is_refused() {
    let rows: Vec<(Vec<String>, String, Selected)> = cases("selection");
    for (input, form, expected) in rows {
        let kind = if form == "path" {
            FormKind::Path
        } else {
            FormKind::Since
        };
        let found = PairSelection::from_names(&input, kind);
        match expected {
            Selected::Chosen(chosen) => {
                let selection = found.unwrap_or_else(|refusal| panic!("{input:?}: {refusal}"));
                assert_eq!(names(selection), chosen, "{input:?} {form}");
            }
            Selected::Refused(text) => assert_eq!(found, Err(text), "{input:?} {form}"),
        }
    }
}

#[test]
fn a_selection_of_no_check_is_refused() {
    let refused = PairSelection::of(&[]).expect_err("an empty selection");
    assert!(refused.starts_with("no check selected."), "{refused}");
}

#[test]
fn a_selection_holds_each_check_once() {
    let selection =
        PairSelection::of(&[PairCheck::CloneDelta, PairCheck::CloneDelta]).expect("a selection");
    assert_eq!(names(selection), ["clone_delta"]);
}
