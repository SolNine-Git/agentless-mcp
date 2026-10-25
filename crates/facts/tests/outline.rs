//! The key outline of a data file: the levels it lists, the cap, the value shapes, and the
//! count of every key.

use std::sync::Arc;

use facts::{DataOutline, OUTLINE_CAP, data_outline};
use syntax::{Language, parse};

fn outline(language: Language, source: &str) -> DataOutline {
    let parsed = parse(language, Arc::from(source)).expect("the source parses");
    data_outline(&parsed).expect("a data language has an outline")
}

fn rows(outline: &DataOutline) -> Vec<(u8, String)> {
    let rows = outline.rows.iter();
    rows.map(|row| (row.depth, row.symbol.signature.clone()))
        .collect()
}

#[test]
fn the_outline_stops_at_level_two_and_at_lists() {
    let source = r#"{"a": {"b": {"c": 1}}, "l": [{"x": 1}], "s": "v"}"#;
    let found = outline(Language::Json, source);
    let expected = [
        (1, "a = object, 1 key"),
        (2, "b = object, 1 key"),
        (1, "l = list, 1 item"),
        (1, "s = \"v\""),
    ];
    let expected: Vec<(u8, String)> = expected
        .iter()
        .map(|(depth, signature)| (*depth, (*signature).to_owned()))
        .collect();
    assert_eq!(rows(&found), expected);
    assert_eq!((found.keys, found.root), (6, None));
}

#[test]
fn a_toml_table_header_is_one_level_and_its_pairs_the_second() {
    let source = "[tool.ruff]\nline-length = 100\n[tool.ruff.lint]\nselect = [\"E\", \"F\"]\n";
    let found = outline(Language::Toml, source);
    let expected = [
        (1, "table tool.ruff, 1 key"),
        (1, "table tool.ruff.lint, 1 key"),
        (2, "line-length = 100"),
        (2, "select = list, 2 items"),
    ];
    let expected: Vec<(u8, String)> = expected
        .iter()
        .map(|(depth, signature)| (*depth, (*signature).to_owned()))
        .collect();
    assert_eq!(rows(&found), expected);
    assert_eq!(found.keys, 4);
}

#[test]
fn the_cap_keeps_every_level_one_key_before_any_level_two_key() {
    let objects: Vec<String> = (0..400)
        .map(|at| format!("\"k{at}\": {{\"x\": 1, \"y\": 2}}"))
        .collect();
    let source = format!("{{\n{}\n}}\n", objects.join(",\n"));
    let found = outline(Language::Json, &source);
    let ones = found.rows.iter().filter(|row| row.depth == 1).count();
    assert_eq!(
        (found.rows.len(), ones, found.keys),
        (OUTLINE_CAP, 400, 1200)
    );
    let last_two = found.rows.iter().rfind(|row| row.depth == 2);
    assert_eq!(
        last_two.map(|row| row.symbol.qualname()),
        Some("k49.y".to_owned())
    );
}

#[test]
fn a_scalar_value_shows_its_first_forty_characters() {
    let found = outline(
        Language::Json,
        &format!("{{\"k\": \"{}\"}}", "a".repeat(60)),
    );
    let signature = &found.rows[0].symbol.signature;
    assert_eq!(signature, &format!("k = \"{}...", "a".repeat(36)));
}

#[test]
fn a_scalar_value_shows_as_the_file_writes_it_with_its_spaces() {
    let source = r#"{"repo": "   repo: {root}", "tab": "a\tb  c", "empty": "", "none": null}"#;
    let found = outline(Language::Json, source);
    let expected = [
        (1, r#"repo = "   repo: {root}""#),
        (1, r#"tab = "a\tb  c""#),
        (1, r#"empty = """#),
        (1, "none = null"),
    ];
    let expected: Vec<(u8, String)> = expected
        .iter()
        .map(|(depth, signature)| (*depth, (*signature).to_owned()))
        .collect();
    assert_eq!(rows(&found), expected);
}

#[test]
fn a_yaml_key_with_no_value_shows_null_and_a_newline_shows_as_backslash_n() {
    let source =
        "on:\n  push:\n  pull_request: # every branch\nrun: |\n  cargo  test\n  cargo fmt\nq: ''\n";
    let found = outline(Language::Yaml, source);
    let expected = [
        (1, "on = object, 2 keys"),
        (2, "push = null"),
        (2, "pull_request = null"),
        (1, r"run = |\n  cargo  test\n  cargo fmt"),
        (1, "q = ''"),
    ];
    let expected: Vec<(u8, String)> = expected
        .iter()
        .map(|(depth, signature)| (*depth, (*signature).to_owned()))
        .collect();
    assert_eq!(rows(&found), expected);
}

#[test]
fn a_long_key_and_its_value_stay_within_the_signature_cap() {
    let key = "k".repeat(90);
    let found = outline(Language::Json, &format!("{{\"{key}\": 1}}"));
    let signature = &found.rows[0].symbol.signature;
    assert_eq!(signature, &format!("{}...", "k".repeat(77)));
}

#[test]
fn a_root_list_gives_its_shape_and_no_row() {
    let json = outline(Language::Json, "[{\"id\": 1}, {\"id\": 2}, {\"id\": 3}]");
    assert_eq!((json.rows.len(), json.keys), (0, 3));
    assert_eq!(json.root.as_deref(), Some("list, 3 items"));
    let yaml = outline(Language::Yaml, "- name: a\n- name: b\n");
    assert_eq!((yaml.rows.len(), yaml.keys), (0, 2));
    assert_eq!(yaml.root.as_deref(), Some("list, 2 items"));
}

#[test]
fn a_yaml_anchor_does_not_hide_the_mapping_it_names() {
    let found = outline(Language::Yaml, "a: &base\n  b: 1\nc: *base\n");
    assert_eq!(
        rows(&found),
        [
            (1, "a = object, 1 key".to_owned()),
            (2, "b = 1".to_owned()),
            (1, "c = *base".to_owned())
        ]
    );
}

#[test]
fn only_a_language_with_data_keys_has_an_outline() {
    let parsed = parse(Language::Python, Arc::from("x = 1\n")).expect("the source parses");
    assert_eq!(data_outline(&parsed), None);
}
