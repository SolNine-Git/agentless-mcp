//! The parser, lexer and near-miss cases against the reference results, the gap texts of each
//! parse refusal, and the recorded TOML deviations.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::Arc;

use checks::{Pair, SideText, TextPair, contract_drift};
use graph::{FileFacts, NameIndex, Resolver, Scopes};
use patch::Edit;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use syntax::{Language, Unparsable};

use crate::dangling::near_misses;
use crate::fragment::{path_name, suffix};
use crate::lexer::{
    literal_end, parameter_shape, positional_arguments, signature_names, structure,
};
use crate::manifest::{normalize_distribution, python_floor, requirement_name};
use crate::{
    CHECK_COVERAGE, CHECK_CYCLE_DELTA, DeclaredDependencies, LintReport, ManifestParse, Parse,
    RepoFacts, gap, lint_with, parse_pyproject_dependencies, parse_requirements,
};

const DID_NOT_PARSE: &str = "pyproject.toml did not parse: ";
const TOML_REFUSAL: &str = "pyproject.toml did not parse: TOMLDecodeError: ";

fn cases<T: DeserializeOwned>(section: &str) -> Vec<T> {
    text::testing::cases(env!("CARGO_MANIFEST_DIR"), "units", section)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case<T> {
    text: String,
    expect: T,
}

// Checks `run` on each case of `section`, which holds at least `minimum` cases.
fn each_case<T>(section: &str, minimum: usize, run: impl Fn(&str) -> T)
where
    T: DeserializeOwned + PartialEq + Debug,
{
    let all: Vec<Case<T>> = cases(section);
    for case in &all {
        assert_eq!(run(&case.text), case.expect, "{section}: {:?}", case.text);
    }
    assert!(all.len() >= minimum, "{section}: {} cases", all.len());
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ManifestRow {
    packages: Vec<String>,
    warnings: Vec<String>,
    parsed: bool,
    requires_python: String,
}

fn row(parse: ManifestParse) -> ManifestRow {
    ManifestRow {
        packages: parse.packages.into_iter().collect(),
        warnings: parse.warnings,
        parsed: parse.parsed,
        requires_python: parse.requires_python,
    }
}

// Whether `found` reads as the reference: equal when it parsed, else a one-line refusal whose
// text after the class name is the toml crate's own.
fn reads_as_reference(found: &ManifestRow, expected: &ManifestRow) -> bool {
    if expected.parsed {
        return found == expected;
    }
    let refused = |row: &ManifestRow| {
        matches!(row.warnings.as_slice(), [warning] if warning.starts_with(TOML_REFUSAL)
            && !warning.contains('\n'))
    };
    refused(expected)
        && refused(found)
        && !found.parsed
        && found.packages.is_empty()
        && found.requires_python.is_empty()
}

#[test]
fn a_manifest_reads_as_the_reference_reads_it() {
    let all: Vec<Case<ManifestRow>> = cases("parse_pyproject");
    let differing: Vec<String> = all
        .iter()
        .filter_map(|case| {
            let found = row(parse_pyproject_dependencies(&case.text));
            let expected = &case.expect;
            (!reads_as_reference(&found, expected)).then(|| {
                format!(
                    "{:?}\n  found:  {found:?}\n  expect: {expected:?}",
                    case.text
                )
            })
        })
        .collect();
    assert!(differing.is_empty(), "{}", differing.join("\n"));
    assert!(all.len() > 15, "{} cases", all.len());
}

#[test]
fn a_document_only_the_toml_crate_reads_parses() {
    let all: Vec<Case<ManifestRow>> = cases("parse_pyproject_rust_only");
    for case in &all {
        assert!(!case.expect.parsed, "{:?}", case.text);
        assert!(case.expect.warnings[0].starts_with(DID_NOT_PARSE));
        let found = parse_pyproject_dependencies(&case.text);
        assert!(found.parsed, "{:?}: {:?}", case.text, found.warnings);
    }
    assert_eq!(all.len(), 8);
}

#[test]
fn a_document_only_the_reference_reads_is_refused() {
    let all: Vec<Case<ManifestRow>> = cases("parse_pyproject_python_only");
    for case in &all {
        assert!(case.expect.parsed, "{:?}", case.text);
        let found = parse_pyproject_dependencies(&case.text);
        assert!(!found.parsed, "{:?}", case.text);
        let [warning] = found.warnings.as_slice() else {
            panic!("{:?}: {:?}", case.text, found.warnings);
        };
        assert!(warning.starts_with(TOML_REFUSAL), "{warning}");
    }
    assert_eq!(all.len(), 5);
}

#[test]
fn nesting_past_80_levels_is_refused_with_the_crates_message() {
    let nested = format!("a = {}1{}\n", "[".repeat(81), "]".repeat(81));
    let found = parse_pyproject_dependencies(&nested);
    let refusal = "cannot recurse further; max recursion depth met (at line 1, column 85)";
    assert_eq!(found.warnings, [format!("{TOML_REFUSAL}{refusal}")]);
    assert!(!found.parsed);
    let limit = format!("a = {}1{}\n", "[".repeat(80), "]".repeat(80));
    assert!(parse_pyproject_dependencies(&limit).parsed);
}

#[test]
fn a_requirements_file_reads_as_the_reference_reads_it() {
    each_case("parse_requirements", 6, |text| {
        parse_requirements(text).into_iter().collect::<Vec<_>>()
    });
}

#[test]
fn a_name_reads_as_the_reference_reads_it() {
    each_case("requirement_name", 10, requirement_name);
    each_case("normalize_distribution", 6, normalize_distribution);
}

#[test]
fn a_floor_reads_as_the_reference_reads_it() {
    each_case("python_floor", 15, python_floor);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NearMissCase {
    name: String,
    known: Vec<String>,
    expect: Vec<String>,
}

#[test]
fn near_misses_rank_as_the_reference_ranks_them() {
    let all: Vec<NearMissCase> = cases("near_misses");
    for case in &all {
        let known: Vec<&str> = case.known.iter().map(String::as_str).collect();
        assert_eq!(
            near_misses(&case.name, &known),
            case.expect,
            "{}",
            case.name
        );
    }
    assert!(all.len() > 10);
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArgumentCase {
    text: String,
    offset: usize,
    expect: Option<Vec<String>>,
}

#[test]
fn a_call_reads_as_the_reference_scanner_reads_it() {
    let all: Vec<ArgumentCase> = cases("positional_arguments");
    for case in &all {
        let chars: Vec<char> = case.text.chars().collect();
        let found = positional_arguments(&chars, &structure(&chars), case.offset);
        assert_eq!(found, case.expect, "{:?}", case.text);
    }
    assert!(all.len() > 50);
}

#[test]
fn a_signature_reads_as_the_reference_scanner_reads_it() {
    each_case("parameter_shape", 20, |text| {
        parameter_shape(text).map(|shape| (shape.required, shape.total))
    });
    each_case("signature_names", 6, |text| {
        signature_names(text).into_iter().collect::<Vec<_>>()
    });
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiteralCase {
    text: String,
    start: usize,
    expect: usize,
}

#[test]
fn a_literal_ends_where_the_reference_ends_it() {
    let all: Vec<LiteralCase> = cases("literal_end");
    for case in &all {
        let chars: Vec<char> = case.text.chars().collect();
        assert_eq!(
            literal_end(&chars, case.start),
            case.expect,
            "{:?}",
            case.text
        );
    }
    assert!(all.len() > 25);
}

#[test]
fn a_deeply_nested_fstring_is_scanned_without_recursion() {
    let depth = 200_000;
    let text = format!("{}x{}", "f'{".repeat(depth), "}'".repeat(depth));
    let chars: Vec<char> = text.chars().collect();
    assert_eq!(literal_end(&chars, 1), chars.len());
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SuffixCase {
    path: String,
    suffix: String,
    language: Option<String>,
}

#[test]
fn a_suffix_and_its_language_are_the_references() {
    let all: Vec<SuffixCase> = cases("suffix");
    for case in &all {
        let name = path_name(&case.path);
        assert_eq!(suffix(name), case.suffix, "{:?}", case.path);
        let language = Language::from_path(name).map(|language| language.name().to_owned());
        assert_eq!(language, case.language, "{:?}", case.path);
    }
    assert!(all.len() > 10);
}

const APP: &str = "UNPARSABLE = 1\nCONSTANT = 1\n";

fn scanned(path: &str, text: &str) -> FileFacts {
    let parsed = syntax::parse(Language::Python, Arc::from(text)).expect("the fixture parses");
    FileFacts {
        path: path.to_owned(),
        language: Language::Python,
        line_count: 2,
        facts: facts::extract(&parsed),
    }
}

// The report of one edit to app.py when `parse` stands in for the parser.
fn report_with(parse: &Parse) -> LintReport {
    let files = vec![scanned("app.py", APP)];
    let index = NameIndex::build(&files);
    let scopes = Scopes::build(&files);
    let resolver = Resolver::new(&index, &scopes);
    let texts = BTreeMap::from([("app.py".to_owned(), APP.to_owned())]);
    let facts = RepoFacts::new(&resolver, &texts, DeclaredDependencies::default());
    let edit = Edit {
        index: 0,
        path: "app.py".to_owned(),
        search: "CONSTANT = 1".to_owned(),
        replace: "CONSTANT = 2".to_owned(),
    };
    lint_with(&[edit], &facts, parse)
}

fn refusals() -> [(Unparsable, &'static str); 4] {
    let language = Language::Python;
    [
        (Unparsable::Abi { language, abi: 99 }, "LanguageUnavailable"),
        (Unparsable::NoTree { language }, "ValueError"),
        (Unparsable::TooLarge { len: 7 }, "ValueError"),
        (Unparsable::Boundary { language, node: 3 }, "ValueError"),
    ]
}

#[test]
fn a_fragment_the_parser_refuses_is_a_gap_naming_the_class() {
    for (error, class) in refusals() {
        let refused = error.clone();
        let report = report_with(&move |_, _| Err(refused.clone()));
        let expected = gap(
            CHECK_COVERAGE,
            "app.py",
            "not checked: this edit's replacement block could not be parsed".to_owned(),
            format!("{class}: {error}"),
        );
        assert_eq!(report.findings, [expected], "{error:?}");
    }
}

#[test]
fn a_check_the_parser_refuses_is_a_gap_naming_the_class() {
    for (error, class) in refusals() {
        let refused = error.clone();
        // Only the patched file holds the marker, so the fragments parse and cycle_delta does not.
        let report = report_with(&move |language, source: Arc<str>| {
            if source.contains("UNPARSABLE") {
                return Err(refused.clone());
            }
            syntax::parse(language, source)
        });
        let expected = gap(
            CHECK_CYCLE_DELTA,
            "",
            format!("not checked: the check could not run over this patch ({class})"),
            error.to_string(),
        );
        let cycles: Vec<_> = report
            .findings
            .into_iter()
            .filter(|finding| finding.check == CHECK_CYCLE_DELTA)
            .collect();
        assert_eq!(cycles, [expected], "{error:?}");
    }
}

#[test]
fn a_gap_has_the_shape_of_a_pair_checks_not_checked_row() {
    let pairs = Pair::measure_all(
        vec![TextPair {
            path: "a.py".to_owned(),
            old: Some("x = 1\n".to_owned()),
            new: Some("x = 2\n".to_owned()),
        }],
        |texts| texts.iter().map(SideText::measure).collect(),
    );
    let pairs: Vec<&Pair> = pairs.iter().collect();
    let outcome = contract_drift(&pairs, &[]).expect("a run without globs has a row");
    let reference = serde_json::to_value(&outcome.findings[0].finding).expect("a row serializes");
    let ours = gap(CHECK_COVERAGE, "", "m".to_owned(), "e".to_owned());
    let ours = serde_json::to_value(&ours).expect("a gap serializes");
    let keys = |value: &serde_json::Value| -> Vec<String> {
        value
            .as_object()
            .map(|row| row.keys().cloned().collect())
            .unwrap_or_default()
    };
    assert_eq!(keys(&ours), keys(&reference));
    for field in ["severity", "path", "line", "location"] {
        assert_eq!(ours[field], reference[field], "{field}");
    }
}
