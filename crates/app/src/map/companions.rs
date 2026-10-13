//! The test files that exercise the ranked and seeded files: a walk against the edges, because a
//! test is a pure source that the ranking cannot place.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use facts::Symbol;
use graph::{FileFacts, FileGraph, LineOwners, NameIndex, Reached, flood_backward};
use kernel::Bounded;
use metrics::compensated_sum;

use crate::ops::limits::{MAX_TEST_COMPANIONS, TEST_COMPANION_DEPTH};
use crate::scope::is_test_path;

/// The test files a map lists, how many there were, and whether the walk stopped early.
#[derive(Clone, Debug, PartialEq)]
pub struct Companions {
    /// The test files the cap kept, best first, and how many there were.
    pub rows: Bounded<Companion>,
    /// Whether the walk hit its node bound, which makes the total a floor.
    pub exhausted: bool,
}

/// One test file, the span in it that references the files above, and what the file covers.
#[derive(Clone, Debug, PartialEq)]
pub struct Companion {
    /// The test file's path.
    pub path: String,
    /// The span's first line.
    pub start: u32,
    /// The span's last line.
    pub end: u32,
    /// The ranked, seeded or nearer files that the whole test file references, sorted.
    pub covers: Vec<String>,
    /// The hops from the ranked files.
    pub depth: u32,
    /// The edge weight from the test file into the files it came through.
    pub weight: f64,
}

// One row per test file; ordered by depth, the targets covered, the weight, then the path.
pub(super) fn companions_for(
    graph: &FileGraph<'_>,
    by_path: &BTreeMap<&str, &FileFacts>,
    index: &NameIndex<'_>,
    targets: &BTreeSet<&str>,
) -> Companions {
    let reach = flood_backward(graph, targets.iter().copied(), TEST_COMPANION_DEPTH);
    let nearer = nearer_by_depth(targets, &reach.reached);
    let tests = reach.reached.iter().filter(|row| is_test_path(row.path));
    let mut rows: Vec<Companion> = tests
        .filter_map(|row| {
            let closer = &nearer[&row.depth];
            let file = by_path
                .get(row.path)
                .expect("a reached file is a scanned file");
            let (covers, (start, end)) = reference(file, index, closer)?;
            let weights: Vec<f64> = (closer.iter())
                .map(|other| graph.weight(row.path, other).unwrap_or(0.0))
                .collect();
            Some(Companion {
                path: row.path.to_owned(),
                start,
                end,
                covers,
                depth: row.depth,
                weight: compensated_sum(&weights),
            })
        })
        .collect();
    let covered = |row: &Companion| {
        let named = row
            .covers
            .iter()
            .filter(|path| targets.contains(path.as_str()));
        named.count()
    };
    rows.sort_by(|a, b| {
        let weight = b.weight.partial_cmp(&a.weight).unwrap_or(Ordering::Equal);
        (a.depth.cmp(&b.depth))
            .then(covered(b).cmp(&covered(a)))
            .then(weight)
            .then_with(|| a.path.cmp(&b.path))
    });
    Companions {
        rows: Bounded::cut(rows, MAX_TEST_COMPANIONS),
        exhausted: reach.exhausted,
    }
}

// What a file at each depth could have come through: the targets and every shallower file.
fn nearer_by_depth<'a>(
    targets: &BTreeSet<&'a str>,
    reached: &[Reached<'a>],
) -> BTreeMap<u32, BTreeSet<&'a str>> {
    let depths: BTreeSet<u32> = reached.iter().map(|row| row.depth).collect();
    let mut nearer = targets.clone();
    let mut by_depth = BTreeMap::new();
    for hops in depths {
        by_depth.insert(hops, nearer.clone());
        nearer.extend(
            reached
                .iter()
                .filter(|row| row.depth == hops)
                .map(|row| row.path),
        );
    }
    by_depth
}

// Name references found in one file: the closer files they reach, and the spans that hold them.
#[derive(Default)]
struct Evidence<'a> {
    referenced: BTreeSet<&'a str>,
    hits: BTreeMap<(u32, u32), Hit<'a>>,
    loose: Vec<u32>,
}

#[derive(Default)]
struct Hit<'a> {
    reached: BTreeSet<&'a str>,
    sites: usize,
}

impl<'a> Evidence<'a> {
    fn record(&mut self, owner: Option<&Symbol>, line: u32, reached: &BTreeSet<&'a str>) {
        self.referenced.extend(reached);
        let Some(owner) = owner else {
            self.loose.push(line);
            return;
        };
        let hit = self.hits.entry((owner.line, owner.end_line)).or_default();
        hit.reached.extend(reached);
        hit.sites += 1;
    }

    // The span that reaches the most, then has the most sites, then comes first; else the lines
    // of the references outside every symbol.
    fn span(&self) -> Option<(u32, u32)> {
        let best = self.hits.iter().max_by(|(a, left), (b, right)| {
            (left.reached.len().cmp(&right.reached.len()))
                .then(left.sites.cmp(&right.sites))
                .then(b.cmp(a))
        });
        let loose = || Some((*self.loose.iter().min()?, *self.loose.iter().max()?));
        best.map(|(span, _)| *span).or_else(loose)
    }
}

// The closer files that `file` references by name, and its one span that does it; `None` when it
// references none of them. A same-file definition shadows the name, as it does for the edges.
fn reference(
    file: &FileFacts,
    index: &NameIndex<'_>,
    closer: &BTreeSet<&str>,
) -> Option<(Vec<String>, (u32, u32))> {
    let owners = LineOwners::build(&file.facts.symbols);
    let mut reached_by_name: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut evidence = Evidence::default();
    for reference in file
        .facts
        .refs
        .iter()
        .filter(|reference| reference.is_reference())
    {
        let name = reference.name.as_str();
        let reached = reached_by_name.entry(name).or_insert_with(|| {
            let defining = index.defining_paths(&reference.name);
            if defining.contains(&file.path.as_str()) {
                return BTreeSet::new();
            }
            defining
                .iter()
                .copied()
                .filter(|path| closer.contains(path))
                .collect()
        });
        if !reached.is_empty() {
            evidence.record(owners.owner(reference.line), reference.line, reached);
        }
    }
    let span = evidence.span()?;
    let covers = evidence.referenced.into_iter().map(str::to_owned).collect();
    Some((covers, span))
}

#[cfg(test)]
mod tests;
