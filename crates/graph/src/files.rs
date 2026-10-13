//! The file graph: one weighted edge per (referrer, definer) pair of files, with the weights the
//! ranking and the communities read.

use std::collections::{BTreeMap, BTreeSet};

use crate::names::{NameIndex, common_name_damping, name_multiplier};
use crate::scan::{FileFacts, in_name_graph};
use crate::scopes::{ImportScope, Scopes};

const IMPORT_EDGE_WEIGHT: f64 = 3.0;
const RELATION_IMPORT_WEIGHT: f64 = 2.0;
const RELATION_INHERITANCE_WEIGHT: f64 = 3.0;
// The shipped reference weight and the relation table's entry agree today.
const REFERENCE_WEIGHT: f64 = 1.0;
const UNIQUE_MATCH_MULTIPLIER: f64 = 0.25;
const AMBIGUOUS_MATCH_MULTIPLIER: f64 = 0.05;

/// How the file graph weighs its edges: the repository's noise names, and whether relation
/// weights replace the shipped ones.
#[derive(Clone, Copy, Debug)]
pub struct Weighting<'s> {
    pub stoplist: &'s BTreeSet<String>,
    pub relation_weights: bool,
}

/// A weighted directed graph over the scanned files, its edges in insertion order.
#[derive(Clone, Debug)]
pub struct FileGraph<'a> {
    nodes: Vec<&'a str>,
    edges: Vec<(&'a str, &'a str, f64)>,
    pairs: BTreeMap<(&'a str, &'a str), usize>,
}

impl<'a> FileGraph<'a> {
    /// Build the file graph of one scan: name references, then imports, then (with relation
    /// weights) base classes, file by file.
    ///
    /// # Panics
    ///
    /// Panics when `index` and `scopes` were built from different file slices.
    #[must_use]
    pub fn build(
        index: &NameIndex<'a>,
        scopes: &Scopes<'a>,
        weighting: &Weighting<'_>,
    ) -> FileGraph<'a> {
        assert!(
            std::ptr::eq(index.files(), scopes.files()),
            "the name index and the import scopes come from different scans"
        );
        let files = index.files();
        let mut nodes: Vec<&'a str> = files.iter().map(|file| file.path.as_str()).collect();
        nodes.sort_unstable();
        let mut graph = FileGraph {
            nodes,
            edges: Vec::new(),
            pairs: BTreeMap::new(),
        };
        for (position, file) in files.iter().enumerate() {
            for (target, weight) in graph.file_edges(file, index, scopes.at(position), weighting) {
                graph.add(&file.path, target, weight);
            }
        }
        graph
    }

    /// Return the scanned paths, in path order.
    #[must_use]
    pub fn nodes(&self) -> &[&'a str] {
        &self.nodes
    }

    /// Return each `(source, target, weight)` edge, in the order the build first added it.
    pub fn edges(&self) -> impl Iterator<Item = (&'a str, &'a str, f64)> + '_ {
        self.edges.iter().copied()
    }

    /// Return how many edges the graph holds.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Return the weight of the edge from `source` to `target`, if there is one.
    #[must_use]
    pub fn weight(&self, source: &str, target: &str) -> Option<f64> {
        self.pairs
            .get(&(source, target))
            .map(|&at| self.edges[at].2)
    }

    pub(crate) fn position(&self, path: &str) -> Option<usize> {
        self.nodes.binary_search(&path).ok()
    }

    #[cfg(test)]
    pub(crate) fn from_parts(nodes: Vec<&'a str>, edges: &[(&'a str, &'a str, f64)]) -> Self {
        assert!(
            nodes.windows(2).all(|pair| pair[0] < pair[1]),
            "nodes are sorted and distinct"
        );
        let mut graph = FileGraph {
            nodes,
            edges: Vec::new(),
            pairs: BTreeMap::new(),
        };
        for &(source, target, weight) in edges {
            assert!(graph.position(source).is_some() && graph.position(target).is_some());
            graph.add(source, target, weight);
        }
        graph
    }

    // One file's contributions in accumulation order: references, imports, then base classes.
    fn file_edges(
        &self,
        file: &'a FileFacts,
        index: &NameIndex<'a>,
        scope: &ImportScope<'a>,
        weighting: &Weighting<'_>,
    ) -> Vec<(&'a str, f64)> {
        let mut edges = Vec::new();
        if in_name_graph(file) {
            edges = self.reference_contributions(file, index, weighting.stoplist);
        }
        let import_weight = if weighting.relation_weights {
            RELATION_IMPORT_WEIGHT
        } else {
            IMPORT_EDGE_WEIGHT
        };
        let imports = import_targets(scope, &file.path).into_iter();
        edges.extend(imports.map(|target| (target, import_weight)));
        if weighting.relation_weights {
            let inherited = self.inheritance_targets(file, index).into_iter();
            edges.extend(inherited.map(|target| (target, RELATION_INHERITANCE_WEIGHT)));
        }
        edges
    }

    // The first add of a pair starts from 0.0, as the accumulation it ports does.
    fn add(&mut self, source: &str, target: &'a str, weight: f64) {
        let source = self.nodes[self
            .position(source)
            .expect("an edge starts at a scanned file")];
        if let Some(&at) = self.pairs.get(&(source, target)) {
            self.edges[at].2 += weight;
        } else {
            self.pairs.insert((source, target), self.edges.len());
            self.edges.push((source, target, 0.0 + weight));
        }
    }

    // One (target, contribution) per name-reference edge: names in sorted order, each name's
    // defining files together, so the floats accumulate in one sequence.
    fn reference_contributions(
        &self,
        file: &'a FileFacts,
        index: &NameIndex<'a>,
        stoplist: &BTreeSet<String>,
    ) -> Vec<(&'a str, f64)> {
        let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
        for reference in file
            .facts
            .refs
            .iter()
            .filter(|reference| reference.is_reference())
        {
            *counts.entry(reference.name.as_str()).or_insert(0) += 1;
        }
        let mut contributions = Vec::new();
        for (name, count) in counts {
            let targets = index.defining_paths(name);
            // A same-file definition shadows repository-wide name matches.
            if targets.binary_search(&file.path.as_str()).is_ok() {
                continue;
            }
            let spread = || index.edge_spread(name);
            let contribution =
                REFERENCE_WEIGHT * reference_weight(name, count, stoplist, targets.len(), spread);
            if contribution <= 0.0 {
                continue;
            }
            let known = targets
                .iter()
                .filter(|target| self.position(target).is_some());
            contributions.extend(known.map(|target| (*target, contribution)));
        }
        contributions
    }

    // One target per base per declaring symbol, repeats included.
    fn inheritance_targets(&self, file: &'a FileFacts, index: &NameIndex<'a>) -> Vec<&'a str> {
        let names = file.facts.symbols.iter().flat_map(|symbol| &symbol.bases);
        let mut targets = Vec::new();
        for name in names
            .map(|base| facts::base_name(base))
            .filter(|name| !name.is_empty())
        {
            let defining = index.defining_paths(name);
            // A same-file definition shadows repository-wide name matches here too.
            if defining.binary_search(&file.path.as_str()).is_err() {
                let known = defining
                    .iter()
                    .filter(|target| self.position(target).is_some());
                targets.extend(known.copied());
            }
        }
        targets
    }
}

// count * multiplier * evidence / damping, left to right; no candidate weighs nothing.
fn reference_weight(
    name: &str,
    count: u32,
    stoplist: &BTreeSet<String>,
    candidates: usize,
    spread: impl Fn() -> u32,
) -> f64 {
    if candidates < 1 {
        return 0.0;
    }
    let evidence = if candidates == 1 {
        UNIQUE_MATCH_MULTIPLIER
    } else {
        AMBIGUOUS_MATCH_MULTIPLIER
    };
    f64::from(count) * name_multiplier(name, stoplist) * evidence / common_name_damping(spread())
}

// Per statement, its distinct targets in row order (its own, then each named submodule), with
// the importing file itself left out.
fn import_targets<'a>(scope: &ImportScope<'a>, path: &str) -> Vec<&'a str> {
    let mut targets: Vec<&'a str> = Vec::new();
    for rows in scope
        .statements
        .chunk_by(|left, right| left.statement == right.statement)
    {
        let start = targets.len();
        let resolved = rows
            .iter()
            .filter_map(|row| row.resolved.filter(|target| *target != path));
        for target in resolved {
            if !targets[start..].contains(&target) {
                targets.push(target);
            }
        }
    }
    targets
}
