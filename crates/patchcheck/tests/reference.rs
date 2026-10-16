//! Each lint case gives the reference report, compared as the JSON text.

use std::collections::BTreeMap;
use std::sync::Arc;

use graph::{FileFacts, NameIndex, Resolver, Scopes};
use json::{JsonForm, to_text};
use patch::Edit;
use patchcheck::{DeclaredDependencies, RepoFacts, lint_patch};
use serde::Deserialize;
use syntax::Language;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LintCase {
    name: String,
    files: BTreeMap<String, String>,
    scanned: Vec<(String, String)>,
    texts: Vec<String>,
    dependencies: Dependencies,
    edits: Vec<EditRow>,
    expect: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Dependencies {
    packages: Vec<String>,
    sources: Vec<String>,
    warnings: Vec<String>,
    requires_python: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditRow {
    index: i64,
    path: String,
    search: String,
    replace: String,
}

// The scan the reference made: each parsed path in its language, with the facts of one parse.
fn scan(case: &LintCase) -> Vec<FileFacts> {
    case.scanned
        .iter()
        .map(|(path, name)| {
            let language = Language::from_name(name).unwrap_or_else(|| panic!("no {name}"));
            let text = &case.files[path];
            let parsed = syntax::parse(language, Arc::from(text.as_str()))
                .unwrap_or_else(|error| panic!("{}: {path}: {error}", case.name));
            FileFacts {
                path: path.clone(),
                language,
                line_count: u32::try_from(text::splitlines(text, false).len()).unwrap_or(u32::MAX),
                facts: facts::extract(&parsed),
            }
        })
        .collect()
}

// The report of each of `runs` lint runs of the case against one set of facts.
fn reports(case: &LintCase, runs: usize) -> Vec<String> {
    let files = scan(case);
    let index = NameIndex::build(&files);
    let scopes = Scopes::build(&files);
    let resolver = Resolver::new(&index, &scopes);
    let texts: BTreeMap<String, String> = case
        .texts
        .iter()
        .map(|path| (path.clone(), case.files[path].clone()))
        .collect();
    let dependencies = DeclaredDependencies {
        packages: case.dependencies.packages.iter().cloned().collect(),
        sources: case.dependencies.sources.clone(),
        warnings: case.dependencies.warnings.clone(),
        requires_python: case.dependencies.requires_python.clone(),
    };
    let facts = RepoFacts::new(&resolver, &texts, dependencies);
    let edits: Vec<Edit> = case
        .edits
        .iter()
        .map(|row| Edit {
            index: row.index,
            path: row.path.clone(),
            search: row.search.clone(),
            replace: row.replace.clone(),
        })
        .collect();
    (0..runs)
        .map(|_| {
            to_text(&lint_patch(&edits, &facts), JsonForm::Compact).expect("a report serializes")
        })
        .collect()
}

fn lint_cases() -> Vec<LintCase> {
    text::testing::cases(env!("CARGO_MANIFEST_DIR"), "lint", "lint_patch")
}

#[test]
fn each_lint_case_gives_the_reference_report() {
    let all = lint_cases();
    let differing: Vec<String> = all
        .iter()
        .filter_map(|case| {
            let found = reports(case, 1).remove(0);
            (found != case.expect).then(|| {
                format!(
                    "{}\n  found:  {found}\n  expect: {}",
                    case.name, case.expect
                )
            })
        })
        .collect();
    assert!(
        differing.is_empty(),
        "{} of {} cases differ:\n{}",
        differing.len(),
        all.len(),
        differing.join("\n")
    );
    assert!(all.len() > 140, "{} cases", all.len());
}

#[test]
fn a_second_run_against_the_same_facts_gives_the_same_report() {
    let all = lint_cases();
    let case = all
        .iter()
        .find(|case| case.name == "two_edits_with_an_unknown_language")
        .expect("the case is in the fixture");
    let runs = reports(case, 2);
    assert_eq!(runs[0], case.expect);
    assert_eq!(runs[1], case.expect);
}
