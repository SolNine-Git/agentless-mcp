//! Each generated case measures to the units, blocks, and SLOC lines, or the refusal, it records.

use std::path::Path;
use std::sync::Arc;

use kernel::Key;
use metrics::{
    Block, FileMetrics, TokenizeError, TokenizeErrorKind, Unit, Unmeasurable, measure, stamp,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use syntax::{Language, Tier, parse};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    source: String,
    error: Option<Refusal>,
    units: Option<Vec<UnitRow>>,
    blocks: Option<Vec<BlockRow>>,
    sloc_lines: Option<Vec<u32>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Refusal {
    Parse(String, u32),
    Tokenize(String, String, String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnitRow {
    qualname: String,
    start_byte: u32,
    end_byte: u32,
    start_line: u32,
    body_key: Key,
    cc: u32,
    sloc: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockRow {
    start_line: u32,
    end_line: u32,
    key: Key,
    exact: Key,
    function: bool,
    tokens: u32,
}

#[test]
fn every_case_measures_as_recorded() {
    let mut failures = Vec::new();
    let mut count = 0;
    for language in Language::ALL.into_iter().filter(|l| l.tier() == Tier::One) {
        let cases: Vec<Case> = read(language.name());
        for case in cases {
            count += 1;
            let parsed = parse(language, Arc::from(case.source.as_str()))
                .unwrap_or_else(|error| panic!("{} {}: {error}", language.name(), case.name));
            let found = measure(&parsed);
            let expected = expected(&case);
            if found != expected {
                failures.push(format!(
                    "{} {}:\n  expected {expected:?}\n  found    {found:?}",
                    language.name(),
                    case.name
                ));
            }
        }
    }
    assert!(count > 0);
    assert!(
        failures.is_empty(),
        "{} of {count} cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_language_has_its_recorded_stamp() {
    let stamps: Vec<(String, Option<String>)> = read("stamps");
    assert_eq!(stamps.len(), Language::ALL.len());
    for (name, expected) in stamps {
        let language = Language::from_name(&name).unwrap_or_else(|| panic!("unknown {name}"));
        assert_eq!(stamp(language), expected.as_deref(), "{name}");
    }
}

#[test]
fn a_language_with_no_stamp_is_unsupported() {
    let stamps: Vec<(String, Option<String>)> = read("stamps");
    let unstamped: Vec<Language> = stamps
        .iter()
        .filter(|(_, stamp)| stamp.is_none())
        .filter_map(|(name, _)| Language::from_name(name))
        .collect();
    assert!(!unstamped.is_empty());
    for language in unstamped {
        let parsed = parse(language, Arc::from("")).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            measure(&parsed),
            Err(Unmeasurable::Unsupported { language })
        );
    }
}

fn read<T: DeserializeOwned>(file: &str) -> T {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(format!("{file}.json"));
    let shown = path.display();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{shown}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{shown}: {error}"))
}

fn expected(case: &Case) -> Result<FileMetrics, Unmeasurable> {
    if let Some(refusal) = &case.error {
        return Err(unmeasurable(refusal));
    }
    let (Some(units), Some(blocks), Some(sloc_lines)) =
        (&case.units, &case.blocks, &case.sloc_lines)
    else {
        panic!(
            "{}: the case records neither metrics nor a refusal",
            case.name
        );
    };
    Ok(FileMetrics {
        units: units.iter().map(unit).collect(),
        blocks: blocks.iter().map(block).collect(),
        sloc_lines: sloc_lines.clone(),
    })
}

fn unmeasurable(refusal: &Refusal) -> Unmeasurable {
    match refusal {
        Refusal::Parse(tag, line) if tag == "parse" => Unmeasurable::DoesNotParse { line: *line },
        Refusal::Tokenize(tag, class, message) if tag == "tokenize" => {
            Unmeasurable::Tokenizer(TokenizeError {
                kind: tokenize_kind(class),
                message: message.clone(),
            })
        }
        Refusal::Parse(tag, _) | Refusal::Tokenize(tag, _, _) => panic!("unknown refusal {tag}"),
    }
}

fn tokenize_kind(class: &str) -> TokenizeErrorKind {
    [
        TokenizeErrorKind::Token,
        TokenizeErrorKind::Indentation,
        TokenizeErrorKind::Tab,
    ]
    .into_iter()
    .find(|kind| kind.class_name() == class)
    .unwrap_or_else(|| panic!("unknown tokenizer error class {class}"))
}

fn unit(row: &UnitRow) -> Unit {
    Unit {
        qualname: row.qualname.clone(),
        start_byte: row.start_byte,
        end_byte: row.end_byte,
        start_line: row.start_line,
        body_key: row.body_key,
        cc: row.cc,
        sloc: row.sloc,
    }
}

fn block(row: &BlockRow) -> Block {
    Block {
        start_line: row.start_line,
        end_line: row.end_line,
        key: row.key,
        exact: row.exact,
        function: row.function,
        tokens: row.tokens,
    }
}
