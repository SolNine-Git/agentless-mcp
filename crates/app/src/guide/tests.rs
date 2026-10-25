use text::CodePointString;

use super::{guide_text, section_names, section_text, unknown_section};
use crate::generated::guide::{GUIDE, SECTIONS};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const FIXTURE: &str = "guide/guide";

// A characterization golden without its receipt line: the answer text that a guide example shows.
fn golden_answer(golden: &str) -> String {
    let path = format!("{MANIFEST_DIR}/../../tests/characterization/goldens/{golden}");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    let (_, answer) = text
        .split_once('\n')
        .unwrap_or_else(|| panic!("{path}: no receipt"));
    answer.to_owned()
}

#[test]
fn the_explain_example_is_the_answer_of_the_repo_py_fixture() {
    let section = section_text("explain").unwrap();
    let answer = golden_answer("graph/repo_py.explain.txt");
    assert!(
        section.contains(&format!("```\n{answer}```\n")),
        "{section}"
    );
}

#[test]
fn the_test_companion_example_is_the_section_of_the_repo_py_tests_map() {
    let golden = golden_answer("map/repo_py_tests.map.txt");
    let start = golden.find("tests exercising the files above:").unwrap();
    let section = section_text("map").unwrap();
    let example = format!("```\n{}```\n", &golden[start..]);
    assert!(section.contains(&example), "{section}");
}

#[test]
fn the_guide_is_the_reference_text() {
    let [text]: [String; 1] = io::testing::cases(MANIFEST_DIR, FIXTURE, "text");
    assert_eq!(guide_text(), text);
}

#[test]
fn the_section_names_are_the_reference_names_in_document_order() {
    let [names]: [Vec<String>; 1] = io::testing::cases(MANIFEST_DIR, FIXTURE, "names");
    assert_eq!(section_names().collect::<Vec<_>>(), names);
}

#[test]
fn each_section_text_is_the_reference_text() {
    let all: Vec<(String, String)> = io::testing::cases(MANIFEST_DIR, FIXTURE, "sections");
    assert!(!all.is_empty());
    for (name, text) in all {
        assert_eq!(section_text(&name), Some(text.as_str()), "{name}");
    }
}

// The table's ranges come from the generator, so a range that is not a slice reads as no section.
#[test]
fn every_table_range_is_a_heading_led_slice_of_the_guide() {
    for &(name, start, end) in &SECTIONS {
        let text = GUIDE.get(start..end);
        assert!(text.is_some_and(|text| text.starts_with('#')), "{name}");
    }
}

#[test]
fn each_unknown_name_gets_the_reference_refusal() {
    let all: Vec<(Vec<u32>, String)> = io::testing::cases(MANIFEST_DIR, FIXTURE, "unknown");
    assert!(!all.is_empty());
    for (codes, expected) in all {
        let mut name = CodePointString::default();
        for code in codes {
            name.push(code);
        }
        if let Some(text) = name.as_str() {
            assert_eq!(section_text(text), None, "{text:?}");
        }
        assert_eq!(unknown_section(&name), expected);
    }
}
