use text::CodePointString;

use super::{guide_text, section_names, section_text, unknown_section};
use crate::generated::guide::{GUIDE, SECTIONS};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const FIXTURE: &str = "guide/guide";

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
