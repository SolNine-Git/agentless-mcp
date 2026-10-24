//! Each generated case measures to the units, blocks, and SLOC lines, or the refusal, it records.

use std::sync::Arc;

use kernel::Key;
use metrics::{
    Block, FileMetrics, Measurable, TokenizeError, TokenizeErrorKind, Unit, Unmeasurable, measure,
    stamp,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use syntax::{Language, Parsed, Tier, parse};
use text::testing::document;

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
    assert_every_case(|parsed| measure(&parsed), |expected| expected);
}

// The entry point that the checks use gives what `measure` gives, apart from the SLOC lines.
#[test]
fn every_case_measures_as_recorded_through_measurable() {
    assert_every_case(
        |parsed| {
            Measurable::new(parsed).map(|measurable| (measurable.units(), measurable.blocks()))
        },
        |expected| expected.map(|metrics| (metrics.units, metrics.blocks)),
    );
}

fn assert_every_case<T: PartialEq + std::fmt::Debug>(
    found: impl Fn(Parsed) -> Result<T, Unmeasurable>,
    shown: impl Fn(Result<FileMetrics, Unmeasurable>) -> Result<T, Unmeasurable>,
) {
    let cases = tier1_cases();
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|(language, case)| {
            let parsed = parse(*language, Arc::from(case.source.as_str()))
                .unwrap_or_else(|error| panic!("{} {}: {error}", language.name(), case.name));
            let (found, expected) = (found(parsed), shown(expected(case)));
            (found != expected).then(|| {
                let name = language.name();
                format!(
                    "{name} {}:\n  expected {expected:?}\n  found    {found:?}",
                    case.name
                )
            })
        })
        .collect();
    assert!(!cases.is_empty());
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

fn tier1_cases() -> Vec<(Language, Case)> {
    Language::ALL
        .into_iter()
        .filter(|language| language.tier() == Tier::One)
        .flat_map(|language| {
            let cases: Vec<Case> = read(language.name());
            cases.into_iter().map(move |case| (language, case))
        })
        .collect()
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
    document(env!("CARGO_MANIFEST_DIR"), file)
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
