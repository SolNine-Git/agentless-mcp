use std::collections::BTreeSet;

use graph::Reached;
use io::Granularity;
use serde::Deserialize;
use text::testing::cases;

use super::{Companions, Evidence, nearer_by_depth};
use crate::IntArg;
use crate::map::tests::{fixture_map, request};

// The reference's listing, each weight as its bits.
#[derive(Debug, Deserialize, PartialEq)]
struct Answer {
    total: usize,
    limit: usize,
    exhausted: bool,
    rows: Vec<(String, u32, u32, Vec<String>, u32, String)>,
}

// The repository, the focus, the file cap, the answer under one hash seed, and the answer under
// the other when the two differ.
type CompanionCase = (String, Vec<String>, usize, Answer, Option<Answer>);

fn answer(companions: &Companions) -> Answer {
    let rows = companions.rows.rows().iter().map(|row| {
        let weight = format!("0x{:016x}", row.weight.to_bits());
        let covers = row.covers.clone();
        (
            row.path.clone(),
            row.start,
            row.end,
            covers,
            row.depth,
            weight,
        )
    });
    Answer {
        total: companions.rows.total(),
        limit: companions.rows.limit(),
        exhausted: companions.exhausted,
        rows: rows.collect(),
    }
}

#[test]
fn the_companions_are_the_reference_companions_under_either_hash_seed() {
    let all: Vec<CompanionCase> = cases(env!("CARGO_MANIFEST_DIR"), "read/companions", "cases");
    assert!(!all.is_empty());
    for (repo, focus, max_files, first, second) in all {
        let mut asked = request(&focus, Granularity::File);
        asked.max_files = Some(IntArg::from_usize(max_files));
        let found = answer(&fixture_map(&repo, &asked).companions);
        let agrees = found == first || second.as_ref().is_some_and(|other| found == *other);
        assert!(agrees, "{repo} {focus:?} {max_files}: {found:?}");
    }
}

#[test]
fn each_depth_comes_through_the_targets_and_every_shallower_file() {
    let targets: BTreeSet<&str> = ["a.py"].into();
    let reached = [
        Reached {
            path: "b.py",
            depth: 1,
        },
        Reached {
            path: "c.py",
            depth: 2,
        },
        Reached {
            path: "d.py",
            depth: 1,
        },
    ];
    let nearer = nearer_by_depth(&targets, &reached);
    assert_eq!(nearer[&1], ["a.py"].into());
    assert_eq!(nearer[&2], ["a.py", "b.py", "d.py"].into());
    assert_eq!(nearer.len(), 2);
}

#[test]
fn the_span_that_reaches_most_wins_then_the_most_sites_then_the_first() {
    let one: BTreeSet<&str> = ["a.py"].into();
    let two: BTreeSet<&str> = ["a.py", "b.py"].into();
    let mut evidence = Evidence::default();
    evidence
        .hits
        .entry((10, 20))
        .or_default()
        .reached
        .extend(&one);
    evidence
        .hits
        .entry((30, 40))
        .or_default()
        .reached
        .extend(&two);
    assert_eq!(evidence.span(), Some((30, 40)));
    evidence
        .hits
        .entry((5, 8))
        .or_default()
        .reached
        .extend(&two);
    assert_eq!(evidence.span(), Some((5, 8)));
    evidence.hits.entry((30, 40)).or_default().sites = 3;
    assert_eq!(evidence.span(), Some((30, 40)));
}

#[test]
fn references_outside_every_symbol_span_their_first_and_last_line() {
    let reached: BTreeSet<&str> = ["a.py"].into();
    let mut evidence = Evidence::default();
    assert_eq!(evidence.span(), None);
    for line in [7, 3, 12] {
        evidence.record(None, line, &reached);
    }
    assert_eq!(evidence.span(), Some((3, 12)));
    assert_eq!(evidence.referenced, reached);
}
