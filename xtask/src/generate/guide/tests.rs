use serde_json::Value;

use super::{SOURCE, Section, grouped, module, sections};
use crate::repo;

const FIXTURE: &str = "crates/app/fixtures/guide/guide.json";

fn fixture() -> Value {
    let root = repo::root().unwrap();
    serde_json::from_str(&repo::read(&root.join(FIXTURE)).unwrap()).unwrap()
}

fn texts<'t>(text: &'t str, found: &[Section]) -> Vec<(String, &'t str)> {
    found
        .iter()
        .map(|section| (section.name.clone(), &text[section.text.clone()]))
        .collect()
}

fn expected_rows(rows: &Value) -> Vec<(String, String)> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let name = row[0].as_str().unwrap().to_owned();
            (name, row[1].as_str().unwrap().to_owned())
        })
        .collect()
}

fn owned(rows: Vec<(String, &str)>) -> Vec<(String, String)> {
    rows.into_iter()
        .map(|(name, text)| (name, text.to_owned()))
        .collect()
}

// Each synthetic guide splits into the reference sections, or is refused for the same names.
#[test]
fn each_synthetic_guide_splits_as_the_reference_says() {
    let document = fixture();
    let cases = document["synthetic"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let text = case[0].as_str().unwrap();
        let expected = &case[1];
        match sections(text) {
            Ok(found) => assert_eq!(
                owned(texts(text, &found)),
                expected_rows(expected),
                "{text:?}"
            ),
            Err(problems) => {
                let names = expected["duplicate"].as_array();
                let names = names.unwrap_or_else(|| panic!("{text:?}: refused: {problems:?}"));
                assert_eq!(problems.len(), names.len(), "{problems:?}");
                for (problem, name) in problems.iter().zip(names) {
                    let quoted = format!("{:?}", name.as_str().unwrap());
                    assert!(problem.contains(&quoted), "{problem}");
                }
            }
        }
    }
}

#[test]
fn the_guide_source_splits_into_the_reference_sections() {
    let document = fixture();
    let text = document["text"][0].as_str().unwrap();
    let root = repo::root().unwrap();
    assert_eq!(repo::read(&root.join(SOURCE)).unwrap(), text);
    let found = sections(text).unwrap();
    assert_eq!(
        owned(texts(text, &found)),
        expected_rows(&document["sections"])
    );
}

#[test]
fn a_line_break_that_is_not_a_newline_is_refused() {
    for refused in [
        "\r", "\u{b}", "\u{c}", "\u{1c}", "\u{1f}", "\u{85}", "\u{2028}", "\0",
    ] {
        let text = format!("## A\nx{refused}y\n");
        let problems = sections(&text).unwrap_err();
        assert!(
            problems[0].contains("is refused"),
            "{refused:?}: {problems:?}"
        );
    }
    assert!(sections("## A\n\tx\n").is_ok());
}

#[test]
fn the_module_holds_the_text_and_a_table_of_every_section() {
    let made = module("# T\n## Alpha\nx\n### Beta\n").unwrap();
    assert!(made.contains("pub(crate) const GUIDE: &str = \"# T\\n## Alpha\\nx\\n### Beta\\n\";"));
    assert!(made.contains("[(&str, usize, usize); 2]"));
    assert!(made.contains("(\"alpha\", 4, 23),"), "{made}");
    assert!(made.contains("(\"beta\", 15, 23),"), "{made}");
    assert!(module("## A\n## a\n").is_err());
}

// clippy refuses a long integer literal with no separators, so a guide past 99,999 bytes still
// gives a module that passes it.
#[test]
fn an_offset_groups_its_digits_by_three() {
    for (offset, want) in [
        (0, "0"),
        (999, "999"),
        (1_000, "1_000"),
        (99_570, "99_570"),
        (100_028, "100_028"),
        (1_234_567, "1_234_567"),
    ] {
        assert_eq!(grouped(offset), want);
    }
}
