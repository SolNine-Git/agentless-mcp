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
fn the_erosion_checks_and_the_tree_checks_are_the_pair_checks() {
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

#[test]
fn each_pair_check_reads_the_table_row_at_its_own_place() {
    for (place, check) in PairCheck::ALL.into_iter().enumerate() {
        assert_eq!(check as usize, place, "{check:?}");
        let variant = format!("{check:?}").to_lowercase();
        assert_eq!(check.name().replace('_', ""), variant, "{check:?}");
    }
}

#[test]
fn each_pair_check_keeps_the_paths_of_its_rule() {
    let paths = [
        "src/app.py",
        "tests/test_app.py",
        "src/fixtures/sample.py",
        "README.md",
    ];
    let found: Vec<(&str, [bool; 4])> = PairCheck::ALL
        .map(|check| (check.name(), paths.map(|path| check.keeps(path))))
        .to_vec();
    let expected = [
        ("complexity_delta", [true, true, false, true]),
        ("contract_drift", [true, false, false, true]),
        ("clone_delta", [true, false, false, false]),
        ("callee_overlap", [true, false, false, false]),
        ("cycle_dodge", [true, false, false, false]),
    ];
    assert_eq!(found, expected);
}

// A row that a form does not run names its own refusal, so the refusal never falls back to the text
// about patch checks.
#[test]
fn each_pair_check_that_a_form_does_not_run_has_its_own_refusal() {
    for check in PairCheck::ALL {
        for form in [FormKind::Path, FormKind::Since] {
            let refused = !check.runs_in(form);
            assert_eq!(
                check.refused_in(form).is_some(),
                refused,
                "{check:?} {form:?}"
            );
        }
    }
    assert!(PairCheck::CalleeOverlap.runs_in(FormKind::Since));
    assert!(PairCheck::CycleDodge.runs_in(FormKind::Since));
}

// callee_overlap reads Rust and Python source files, and cycle_dodge Python ones only: the
// languages whose tables hold a callee stoplist, and the one whose walker sets Import.runs.
#[test]
fn each_tree_check_keeps_the_source_files_of_its_languages_and_no_test_or_fixture() {
    let paths = [
        "src/app.rs",
        "src/app.py",
        "src/app.go",
        "src/app.ts",
        "tests/app.rs",
        "tests/app.py",
        "src/fixtures/app.py",
    ];
    let kept = |check: PairCheck| paths.map(|path| check.keeps(path));
    let callee = [true, true, false, false, false, false, false];
    assert_eq!(kept(PairCheck::CalleeOverlap), callee);
    let dodge = [false, true, false, false, false, false, false];
    assert_eq!(kept(PairCheck::CycleDodge), dodge);
}
