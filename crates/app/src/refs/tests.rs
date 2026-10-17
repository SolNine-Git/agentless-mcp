use std::collections::BTreeSet;

use facts::{Facts, Ref, Role};
use graph::{FileFacts, NameIndex, Site, definitions_for, references_to};
use kernel::Stopped;
use syntax::Language;

use super::shared::shared_callers;
use super::{dedupe, names_by_path, round_robin};
use crate::clock::spent;
use crate::endpoints::scanned::{Scanned, TESTED};
use crate::scan::RepoFacts;

fn file(path: &str, lines: &[u32]) -> FileFacts {
    let refs = lines.iter().map(|&line| Ref {
        name: format!("n{line}"),
        line,
        role: Role::Reference,
        qualifier: String::new(),
    });
    FileFacts {
        path: path.to_owned(),
        language: Language::Python,
        line_count: 100,
        facts: Facts {
            refs: refs.collect(),
            ..Facts::default()
        },
    }
}

fn sites(files: &[FileFacts]) -> Vec<Site<'_>> {
    let each = files.iter().flat_map(|file| {
        let refs = file.facts.refs.iter();
        refs.map(move |reference| Site { file, reference })
    });
    each.collect()
}

fn places(sites: &[Site<'_>]) -> Vec<(String, u32)> {
    let places = sites
        .iter()
        .map(|site| (site.path().to_owned(), site.reference.line));
    places.collect()
}

#[test]
fn one_site_per_line_in_file_then_line_order() {
    let files = [file("b.py", &[9, 3, 3]), file("a.py", &[5])];
    let spelled = sites(&files);
    let unique = dedupe(&spelled);
    let expected =
        [("a.py", 5), ("b.py", 3), ("b.py", 9)].map(|(path, line)| (path.to_owned(), line));
    assert_eq!(places(&unique), expected);
    let names = names_by_path(&spelled);
    assert_eq!(names["b.py"].len(), 2);
}

#[test]
fn the_limit_takes_one_site_per_file_in_turn() {
    let files = [
        file("a.py", &[1, 2, 3]),
        file("b.py", &[7]),
        file("c.py", &[4, 8]),
    ];
    let unique = dedupe(&sites(&files));
    let kept = round_robin(&unique, 5);
    let expected = [
        ("a.py", 1),
        ("b.py", 7),
        ("c.py", 4),
        ("a.py", 2),
        ("c.py", 8),
    ];
    let expected = expected.map(|(path, line)| (path.to_owned(), line));
    assert_eq!(places(&kept), expected);
    assert_eq!(round_robin(&unique, 100).len(), 6);
}

#[test]
fn the_shared_caller_ranking_stops_under_a_spent_deadline() {
    let scanned = Scanned::new(&TESTED);
    let loaded = RepoFacts::load(&scanned.services, &scanned.reading());
    let facts = loaded.value.unwrap_or_else(|error| panic!("{error}"));
    let index = NameIndex::build(&facts.files);
    let definitions = definitions_for(&index, "helper");
    let sites: Vec<Site<'_>> = (definitions.iter())
        .flat_map(|definition| references_to(&index, definition))
        .collect();
    let target_ids: BTreeSet<String> = (definitions.iter())
        .map(|definition| definition.stable_id().to_string())
        .collect();

    let ranked = shared_callers(
        &sites,
        &target_ids,
        &index,
        &BTreeSet::new(),
        Some(&spent()),
    );

    assert!(matches!(ranked, Err(Stopped::Spent { .. })), "{ranked:?}");
}
