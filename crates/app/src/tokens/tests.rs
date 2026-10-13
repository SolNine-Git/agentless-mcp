use std::path::PathBuf;

use super::{Chars4, Cl100k, TokenCounter};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

#[test]
fn chars4_counts_code_points_and_rounds_down() {
    assert_eq!(Chars4.count(""), 0);
    assert_eq!(Chars4.count("abc"), 0);
    assert_eq!(Chars4.count("abcd"), 1);
    // Four code points in sixteen bytes are one token, not four.
    assert_eq!(Chars4.count("\u{1f600}\u{1f600}\u{1f600}\u{1f600}"), 1);
    assert_eq!(Chars4.count(&"x".repeat(4001)), 1000);
}

fn golden(path: &str) -> String {
    let path: PathBuf = [MANIFEST_DIR, "../../tests/characterization/goldens", path]
        .iter()
        .collect();
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn cl100k_counts_each_golden_and_its_prefixes_as_the_reference_does() {
    let counter = Cl100k::new().unwrap_or_else(|error| panic!("{error}"));
    let cases: Vec<(String, Option<usize>, usize)> =
        io::testing::cases(MANIFEST_DIR, "read/tokens", "goldens");
    assert!(cases.len() > 200);
    for (path, cut, expected) in cases {
        let text = golden(&path);
        let shown: String = match cut {
            Some(cut) => text.chars().take(cut).collect(),
            None => text,
        };
        assert_eq!(counter.count(&shown), expected, "{path} cut {cut:?}");
    }
}

#[test]
fn cl100k_counts_each_edge_text_as_the_reference_does() {
    let counter = Cl100k::new().unwrap_or_else(|error| panic!("{error}"));
    let cases: Vec<(String, usize)> = io::testing::cases(MANIFEST_DIR, "read/tokens", "texts");
    assert!(!cases.is_empty());
    for (text, expected) in cases {
        assert_eq!(counter.count(&text), expected, "{text:?}");
    }
}
