use serde::Deserialize;
use serde_json::{Value, json};

use super::corpus::{Corpus, ids};
use crate::lookup::{references_to, resolve_definitions};
use crate::names::NameIndex;
use crate::owners::{LineOwners, enclosing_symbol};
use crate::scan::FileFacts;

#[derive(Deserialize)]
struct NameCase {
    name: String,
    definitions: Vec<(String, String)>,
    sites: Vec<(String, u32, String)>,
    defining_paths: Vec<String>,
    reference_spread: Option<u32>,
    caller_spread: u32,
    spelling_spread: u32,
}

#[test]
fn the_name_index_matches_the_reference() {
    for corpus in Corpus::all() {
        let files = corpus.files();
        let index = NameIndex::build(&files);
        let Some(cases) = corpus.section::<NameCase>("index") else {
            continue;
        };
        assert_eq!(index.name_count(), cases.len(), "{}", corpus.name);
        for case in cases {
            check_name(&index, &case, corpus.name);
        }
    }
}

fn check_name(index: &NameIndex<'_>, case: &NameCase, corpus: &str) {
    let name = case.name.as_str();
    let definitions: Vec<(String, String)> = index
        .definitions(name)
        .iter()
        .map(|definition| {
            (
                definition.path().to_owned(),
                definition.stable_id().to_string(),
            )
        })
        .collect();
    let sites: Vec<(String, u32, String)> = index
        .sites(name)
        .iter()
        .map(|site| {
            (
                site.path().to_owned(),
                site.reference.line,
                site.reference.role.as_str().to_owned(),
            )
        })
        .collect();
    let context = format!("{corpus}: {name:?}");
    assert_eq!(definitions, case.definitions, "{context}");
    assert_eq!(sites, case.sites, "{context}");
    assert_eq!(index.defining_paths(name), case.defining_paths, "{context}");
    assert_eq!(
        index.edge_spread(name),
        case.reference_spread.unwrap_or(0),
        "{context}"
    );
    assert_eq!(index.caller_spread(name), case.caller_spread, "{context}");
    assert_eq!(
        index.spelling_spread(name),
        case.spelling_spread,
        "{context}"
    );
}

#[test]
fn lookups_match_the_reference() {
    for corpus in Corpus::all() {
        let files = corpus.files();
        let index = NameIndex::build(&files);
        for (target, expected, scoped) in corpus
            .section::<(String, Value, bool)>("lookups")
            .unwrap_or_default()
        {
            let found = resolve_definitions(&index, &target);
            assert_eq!(
                ids(&found.definitions),
                expected,
                "{}: {target:?}",
                corpus.name
            );
            assert_eq!(found.scoped, scoped, "{}: {target:?}", corpus.name);
        }
    }
}

#[test]
fn references_match_the_reference() {
    for corpus in Corpus::all() {
        let files = corpus.files();
        let index = NameIndex::build(&files);
        let Some(expected) = corpus.section::<Value>("references") else {
            continue;
        };
        let definitions =
            (0..index.name_count()).flat_map(|position| index.definitions_at(position));
        let found: Vec<Value> = definitions
            .map(|definition| {
                let sites: Vec<Value> = references_to(&index, definition)
                    .iter()
                    .map(|site| {
                        json!([
                            site.path(),
                            site.reference.line,
                            site.reference.role.as_str()
                        ])
                    })
                    .collect();
                json!([definition.stable_id().to_string(), sites])
            })
            .collect();
        assert_eq!(found, expected, "{}", corpus.name);
    }
}

#[derive(Deserialize)]
struct OwnerCase {
    path: String,
    enclosing: Vec<Option<String>>,
    owners: Vec<(u32, String)>,
}

#[test]
fn line_owners_match_the_reference() {
    for corpus in Corpus::all() {
        let files = corpus.files();
        for case in corpus.section::<OwnerCase>("owners").unwrap_or_default() {
            let file = files
                .iter()
                .find(|file| file.path == case.path)
                .expect("a corpus file");
            check_owners(file, &case, corpus.name);
        }
    }
}

fn check_owners(file: &FileFacts, case: &OwnerCase, corpus: &str) {
    let id =
        |symbol: &facts::Symbol| facts::StableId::of(file.language, &file.path, symbol).to_string();
    let symbols = &file.facts.symbols;
    let enclosing: Vec<Option<String>> = (0..=file.line_count + 1)
        .map(|line| enclosing_symbol(symbols, line).map(id))
        .collect();
    assert_eq!(enclosing, case.enclosing, "{corpus}: {}", file.path);
    let owners = LineOwners::build(symbols);
    let last = symbols
        .iter()
        .map(|symbol| symbol.end_line)
        .max()
        .unwrap_or(0);
    let found: Vec<(u32, String)> = (0..=last + 1)
        .filter_map(|line| owners.owner(line).map(|symbol| (line, id(symbol))))
        .collect();
    assert_eq!(found, case.owners, "{corpus}: {}", file.path);
}
