//! The views of the fixtures: line counts, slices with their sticky headers, location
//! resolution with every reason text, unparsed mentions, stable ids and ordinals, and the
//! recorded deviations for digits outside ASCII.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use facts::{
    FileSymbols, LocTarget, StableId, Symbol, SymbolKind, extract, line_count, resolve_locs,
    slice_lines, split_ordinal, unparsed_mentions,
};
use serde::Deserialize;
use serde_json::Value;
use syntax::{Language, parse};
use text::testing::cases;

#[derive(Deserialize)]
struct Source {
    name: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocsCase {
    path: String,
    language: String,
    total: u32,
    symbols: Vec<LocSymbol>,
    locs: Vec<String>,
    context: u32,
    stable_ids: Vec<String>,
    spans: Vec<(u32, u32)>,
    intervals: Vec<(u32, u32)>,
    unrecognized: Vec<(String, String)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocSymbol {
    name: String,
    kind: String,
    parent: String,
    line: u32,
    end_line: u32,
    duplicate_index: u32,
}

type SliceCase = (
    String,
    String,
    Vec<(u32, u32)>,
    Vec<(String, u32, u32)>,
    Value,
);

fn section<T: serde::de::DeserializeOwned>(name: &str) -> T {
    cases(env!("CARGO_MANIFEST_DIR"), "views", name)
}

fn source_of(language: &str, name: &str) -> String {
    let file = format!("extract_{language}");
    let all: Vec<Source> = cases(env!("CARGO_MANIFEST_DIR"), &file, "cases");
    let found: HashMap<String, String> = all
        .into_iter()
        .map(|source| (source.name, source.source))
        .collect();
    found
        .get(name)
        .unwrap_or_else(|| panic!("{file} holds no case {name}"))
        .clone()
}

fn symbol(name: &str, kind: &str, line: u32, end_line: u32) -> Symbol {
    Symbol {
        name: name.to_owned(),
        kind: SymbolKind::from_name(kind).unwrap_or_else(|| panic!("an unknown kind {kind}")),
        line,
        end_line,
        name_line: line,
        signature: String::new(),
        parent: String::new(),
        decorated: false,
        bases: Vec::new(),
        is_public: true,
        rationales: Vec::new(),
        duplicate_index: 0,
    }
}

#[test]
fn each_line_count_is_the_reference_count() {
    let all: Vec<(String, u32)> = section("line_count");
    for (text, expected) in all {
        assert_eq!(line_count(&text), expected, "{text:?}");
    }
}

#[test]
fn each_slice_is_the_reference_text_or_refusal() {
    let all: Vec<SliceCase> = section("slice_lines");
    assert!(all.len() > 100);
    let mut sources: HashMap<(String, String), String> = HashMap::new();
    for (language, name, intervals, rows, expected) in all {
        let content = sources
            .entry((language.clone(), name.clone()))
            .or_insert_with(|| source_of(&language, &name));
        let symbols: Vec<Symbol> = rows
            .iter()
            .map(|(kind, line, end)| symbol("s", kind, *line, *end))
            .collect();
        let found = match slice_lines(content, &intervals, &symbols) {
            Ok(text) => Value::from(text),
            Err(refusal) => serde_json::json!({ "refusal": refusal.to_string() }),
        };
        assert_eq!(found, expected, "{name} {intervals:?}");
    }
}

#[test]
fn each_location_set_resolves_as_the_reference_resolves_it() {
    let all: Vec<LocsCase> = section("resolve_locs");
    assert!(all.len() > 40);
    for case in all {
        let symbols: Vec<Symbol> = case
            .symbols
            .iter()
            .map(|row| Symbol {
                parent: row.parent.clone(),
                duplicate_index: row.duplicate_index,
                ..symbol(&row.name, &row.kind, row.line, row.end_line)
            })
            .collect();
        let file = Language::from_name(&case.language).map(|language| FileSymbols {
            language,
            symbols: &symbols,
        });
        let target = LocTarget {
            path: &case.path,
            total_lines: case.total,
            file,
        };
        let found = resolve_locs(&case.locs, &target, case.context);
        let ids: Vec<String> = found.stable_ids.iter().map(ToString::to_string).collect();
        let unrecognized: Vec<(String, String)> = found
            .unrecognized
            .into_iter()
            .map(|item| (item.loc, item.reason))
            .collect();
        let shown = format!("{} context {}", case.path, case.context);
        assert_eq!(ids, case.stable_ids, "{shown}");
        assert_eq!(found.spans, case.spans, "{shown}");
        assert_eq!(found.intervals, case.intervals, "{shown}");
        assert_eq!(unrecognized, case.unrecognized, "{shown}");
    }
}

#[test]
fn each_unparsed_mention_is_on_the_reference_lines() {
    let all: Vec<(String, String, String, Vec<u32>)> = section("unparsed_mentions");
    assert!(all.len() > 100);
    let mut sources: HashMap<(String, String), String> = HashMap::new();
    for (language, name, needle, expected) in all {
        let content = sources
            .entry((language.clone(), name.clone()))
            .or_insert_with(|| source_of(&language, &name));
        let parsed_language = Language::from_name(&language).expect("a known language");
        let parsed = parse(parsed_language, Arc::from(content.as_str())).expect("it parses");
        let errors = extract(&parsed).errors;
        let found = unparsed_mentions(content, &errors, &needle);
        assert_eq!(found, expected, "{name} {needle:?}");
    }
}

#[test]
fn each_stable_id_parses_as_the_reference_parses_it() {
    let all: Vec<(String, Vec<String>)> = section("stable_ids");
    for (text, expected) in all {
        let found = match StableId::from_str(&text) {
            Ok(id) => vec![
                "ok".to_owned(),
                id.prefix().to_owned(),
                id.path().to_owned(),
                id.qualname().to_owned(),
            ],
            Err(error) => vec!["error".to_owned(), error.to_string()],
        };
        assert_eq!(found, expected, "{text:?}");
    }
}

#[test]
fn each_ordinal_splits_as_the_reference_splits_it() {
    let all: Vec<(String, String, u32)> = section("split_ordinal");
    for (text, base, index) in all {
        assert_eq!(split_ordinal(&text), (base.as_str(), index), "{text:?}");
    }
}

// Digits outside ASCII are no line number and no ordinal here, where the reference reads them.
#[test]
fn a_digit_outside_ascii_follows_the_recorded_deviation() {
    let all: Vec<(String, String, Value)> = section("deviations");
    assert_eq!(all.len(), 6);
    for (kind, input, reference) in all {
        if kind == "line" {
            let target = LocTarget {
                path: "a.py",
                total_lines: 10,
                file: None,
            };
            let found = resolve_locs(std::slice::from_ref(&input), &target, 10);
            assert!(found.spans.is_empty(), "{input:?}");
            let reason = &found.unrecognized[0].reason;
            assert!(reason.ends_with(" is not a line number"), "{reason}");
            assert!(reference.is_array(), "{input:?}");
        } else {
            assert_eq!(split_ordinal(&input), (input.as_str(), 0), "{input:?}");
            assert_ne!(reference[0], Value::from(input.clone()), "{input:?}");
        }
    }
}
