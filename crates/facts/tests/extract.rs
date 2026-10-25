//! Every extract case of the fixtures: the symbols with their ids and rationale handles, the
//! imports, the identifier occurrences and the error spans equal the reference output.

use std::fmt::Debug;
use std::sync::Arc;

use facts::{Facts, StableId, extract, rationale_handle};
use serde::Deserialize;
use syntax::{Language, parse};
use text::testing::cases;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    source: String,
    symbols: Vec<SymbolRow>,
    imports: Vec<ImportRow>,
    refs: Vec<(String, u32, String, String)>,
    errors: Vec<(u32, u32)>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SymbolRow {
    name: String,
    kind: String,
    line: u32,
    end_line: u32,
    name_line: u32,
    signature: String,
    parent: String,
    decorated: bool,
    bases: Vec<String>,
    is_public: bool,
    duplicate_index: u32,
    id: String,
    rationales: Vec<RationaleRow>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RationaleRow {
    kind: String,
    text: String,
    line: u32,
    citations: Vec<String>,
    duplicate_index: u32,
    handle: String,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportRow {
    module: String,
    names: Vec<String>,
    local_names: Vec<String>,
    is_relative: bool,
    relative_level: u32,
    line: u32,
    binds_all: bool,
    alias: String,
}

#[test]
fn every_language_gives_the_reference_facts() {
    let mut problems = Vec::new();
    let mut checked = 0;
    for language in Language::ALL {
        let file = format!("extract_{}", language.name());
        let all: Vec<Case> = cases(env!("CARGO_MANIFEST_DIR"), &file, "cases");
        assert!(all.len() >= 6, "{file} holds {} cases", all.len());
        for case in &all {
            checked += 1;
            problems.extend(compare(language, case));
        }
    }
    assert!(checked > 300, "the fixtures hold {checked} cases");
    let shown: Vec<&String> = problems.iter().take(40).collect();
    assert!(
        problems.is_empty(),
        "{} of the cases differ:\n{shown:#?}",
        problems.len()
    );
}

fn compare(language: Language, case: &Case) -> Option<String> {
    let parsed = match parse(language, Arc::from(case.source.as_str())) {
        Ok(parsed) => parsed,
        Err(error) => return Some(format!("{}: does not parse: {error}", case.name)),
    };
    let facts = extract(&parsed);
    let found_symbols = symbol_rows(language, &case.name, &facts);
    first_difference(&case.name, "symbols", &found_symbols, &case.symbols)
        .or_else(|| first_difference(&case.name, "imports", &import_rows(&facts), &case.imports))
        .or_else(|| first_difference(&case.name, "refs", &ref_rows(&facts), &case.refs))
        .or_else(|| first_difference(&case.name, "errors", &error_rows(&facts), &case.errors))
}

fn first_difference<T: PartialEq + Debug>(
    name: &str,
    what: &str,
    found: &[T],
    expected: &[T],
) -> Option<String> {
    let position = found
        .iter()
        .zip(expected)
        .position(|(found, expected)| found != expected);
    match position {
        Some(at) => Some(format!(
            "{name}: {what}[{at}]\n  found    {:?}\n  expected {:?}",
            found[at], expected[at]
        )),
        None if found.len() != expected.len() => Some(format!(
            "{name}: {what}: {} found, {} expected; first extra {:?} / {:?}",
            found.len(),
            expected.len(),
            found.get(expected.len()),
            expected.get(found.len())
        )),
        None => None,
    }
}

fn symbol_rows(language: Language, path: &str, facts: &Facts) -> Vec<SymbolRow> {
    facts
        .symbols
        .iter()
        .map(|symbol| SymbolRow {
            name: symbol.name.clone(),
            kind: symbol.kind.as_str().to_owned(),
            line: symbol.line,
            end_line: symbol.end_line,
            name_line: symbol.name_line,
            signature: symbol.signature.clone(),
            parent: symbol.parent.clone(),
            decorated: symbol.decorated,
            bases: symbol.bases.clone(),
            is_public: symbol.is_public,
            duplicate_index: symbol.duplicate_index,
            id: StableId::of(language, path, symbol).to_string(),
            rationales: symbol
                .rationales
                .iter()
                .map(|rationale| RationaleRow {
                    kind: rationale.kind.as_str().to_owned(),
                    text: rationale.text.clone(),
                    line: rationale.line,
                    citations: rationale.citations.clone(),
                    duplicate_index: rationale.duplicate_index,
                    handle: rationale_handle(language, path, symbol, rationale),
                })
                .collect(),
        })
        .collect()
}

fn import_rows(facts: &Facts) -> Vec<ImportRow> {
    facts
        .imports
        .iter()
        .map(|import| ImportRow {
            module: import.module.clone(),
            names: import.names.clone(),
            local_names: import.local_names.clone(),
            is_relative: import.is_relative,
            relative_level: import.relative_level,
            line: import.line,
            binds_all: import.binds_all,
            alias: import.alias.clone(),
        })
        .collect()
}

fn ref_rows(facts: &Facts) -> Vec<(String, u32, String, String)> {
    facts
        .refs
        .iter()
        .map(|found| {
            (
                found.name.clone(),
                found.line,
                found.role.as_str().to_owned(),
                found.qualifier.clone(),
            )
        })
        .collect()
}

fn error_rows(facts: &Facts) -> Vec<(u32, u32)> {
    facts
        .errors
        .iter()
        .map(|span| (span.start(), span.end()))
        .collect()
}
