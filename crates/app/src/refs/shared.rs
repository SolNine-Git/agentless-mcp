//! The shared callers: the symbols that the target's callers also reference, scored by the files
//! that share them and damped by common names and by promiscuous callers.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use facts::{StableId, Symbol};
use graph::{
    Definition, FileFacts, NameIndex, Site, common_name_damping, enclosing_symbol, name_multiplier,
};
use metrics::compensated_sum;

use super::{CallerRef, SharedCaller};
use crate::is_test_path;

// One symbol that holds a site of the target.
#[derive(Clone, Copy)]
struct Caller<'a> {
    file: &'a FileFacts,
    symbol: &'a Symbol,
}

// One candidate, and the callers that reference it with each caller's fan-out.
struct Adjacency<'a> {
    definition: Definition<'a>,
    callers: Vec<(Caller<'a>, usize)>,
}

// Production candidates rank ahead of every test-defined one; the score orders each part.
pub(super) fn shared_callers(
    sites: &[Site<'_>],
    target_ids: &BTreeSet<String>,
    index: &NameIndex<'_>,
    stoplist: &BTreeSet<String>,
) -> Vec<SharedCaller> {
    let shared = adjacency(&callers(sites), target_ids, index);
    let mut rows: Vec<SharedCaller> = (shared.into_iter())
        .filter(|(_, entry)| entry.callers.len() > 1)
        .map(|(candidate, entry)| scored_row(candidate, &entry, index, stoplist))
        .collect();
    rows.sort_by(ranked);
    rows
}

// The symbols that hold the target's sites, by stable id.
fn callers<'a>(sites: &[Site<'a>]) -> BTreeMap<String, Caller<'a>> {
    let mut callers = BTreeMap::new();
    for site in sites {
        if let Some(symbol) = enclosing_symbol(&site.file.facts.symbols, site.reference.line) {
            let id = StableId::of(site.file.language, site.path(), symbol).to_string();
            callers.insert(
                id,
                Caller {
                    file: site.file,
                    symbol,
                },
            );
        }
    }
    callers
}

// Each candidate that a caller's body references, in first-seen order. A parameter spells its
// own binding, so only references count.
fn adjacency<'a>(
    callers: &BTreeMap<String, Caller<'a>>,
    target_ids: &BTreeSet<String>,
    index: &NameIndex<'a>,
) -> Vec<(String, Adjacency<'a>)> {
    let mut shared: Vec<(String, Adjacency<'a>)> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (caller_id, caller) in callers {
        let names = referenced_names(*caller);
        for definition in names.iter().flat_map(|name| index.definitions(name)) {
            let candidate = definition.stable_id().to_string();
            if target_ids.contains(&candidate) || candidate == *caller_id {
                continue;
            }
            let at = *seen.entry(candidate.clone()).or_insert_with(|| {
                let entry = Adjacency {
                    definition: *definition,
                    callers: Vec::new(),
                };
                shared.push((candidate, entry));
                shared.len() - 1
            });
            shared[at].1.callers.push((*caller, names.len()));
        }
    }
    shared
}

// The distinct names that the caller's body references.
fn referenced_names(caller: Caller<'_>) -> BTreeSet<&str> {
    let (first, last) = (caller.symbol.line, caller.symbol.end_line);
    (caller.file.facts.refs.iter())
        .filter(|reference| (first..=last).contains(&reference.line) && reference.is_reference())
        .map(|reference| reference.name.as_str())
        .collect()
}

// Files vote, not sites, and each file votes with its most focused caller: one over the log
// damping of that caller's fan-out.
fn scored_row(
    candidate: String,
    entry: &Adjacency<'_>,
    index: &NameIndex<'_>,
    stoplist: &BTreeSet<String>,
) -> SharedCaller {
    let symbol = entry.definition.symbol();
    let mut weight_by_file: Vec<(&str, f64)> = Vec::new();
    for (caller, out_degree) in &entry.callers {
        let degree = u32::try_from(*out_degree).unwrap_or(u32::MAX);
        let weight = 1.0 / common_name_damping(degree);
        let path = caller.file.path.as_str();
        match weight_by_file.iter_mut().find(|(held, _)| *held == path) {
            Some((_, best)) => *best = best.max(weight),
            None => weight_by_file.push((path, weight)),
        }
    }
    let weights: Vec<f64> = weight_by_file.iter().map(|(_, weight)| *weight).collect();
    let spread = common_name_damping(index.caller_spread(&symbol.name));
    let score = compensated_sum(&weights) * name_multiplier(&symbol.name, stoplist) / spread;
    let mut callers: Vec<CallerRef> = (entry.callers.iter())
        .map(|(caller, _)| CallerRef {
            qualname: caller.symbol.qualname(),
            path: caller.file.path.clone(),
            line: caller.symbol.line,
        })
        .collect();
    callers.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    SharedCaller {
        stable_id: candidate,
        path: entry.definition.path().to_owned(),
        line: symbol.line,
        overlap: entry.callers.len(),
        shared_files: weight_by_file.len(),
        score,
        callers,
        in_tests: is_test_path(entry.definition.path()),
    }
}

fn ranked(a: &SharedCaller, b: &SharedCaller) -> Ordering {
    let score = b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal);
    (a.in_tests.cmp(&b.in_tests))
        .then(score)
        .then(b.shared_files.cmp(&a.shared_files))
        .then_with(|| a.stable_id.cmp(&b.stable_id))
}
