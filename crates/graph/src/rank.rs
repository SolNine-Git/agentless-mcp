//! Personalized `PageRank` over the file graph, and the ranked order of its files.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use metrics::compensated_sum;

use crate::files::FileGraph;

const DAMPING: f64 = 0.85;
const EPSILON: f64 = 1e-6;
const MAX_ITERATIONS: u32 = 100;
// A reference edge also carries half its weight back towards the referrer in the walk.
const BACKFLOW_WEIGHT: f64 = 0.5;

/// A ranking of the graph's files, and whether the power iteration behind it settled.
#[derive(Clone, Debug, PartialEq)]
pub struct PageRank<'a> {
    nodes: Vec<&'a str>,
    ranks: Vec<f64>,
    support: Vec<bool>,
    converged: bool,
}

impl<'a> PageRank<'a> {
    /// Return whether the iteration settled before its pass limit.
    #[must_use]
    pub fn converged(&self) -> bool {
        self.converged
    }

    /// Return the rank of the file at `path`.
    #[must_use]
    pub fn rank(&self, path: &str) -> Option<f64> {
        self.position(path).map(|at| self.ranks[at])
    }

    /// Return each file with its rank, in path order.
    pub fn ranks(&self) -> impl Iterator<Item = (&'a str, f64)> + '_ {
        self.nodes.iter().copied().zip(self.ranks.iter().copied())
    }

    /// Return whether the walk from the teleport vector's support can arrive at `path`.
    #[must_use]
    pub fn reached(&self, path: &str) -> bool {
        self.position(path).is_some_and(|at| self.support[at])
    }

    fn position(&self, path: &str) -> Option<usize> {
        self.nodes.binary_search(&path).ok()
    }
}

/// Rank `graph`'s files, teleporting to `seeds` (path to share) or uniformly when no seed names
/// a file; an edge out of a pure source carries no weight back into it.
#[must_use]
pub fn pagerank<'a>(
    graph: &FileGraph<'a>,
    seeds: &BTreeMap<String, f64>,
    is_pure_source: impl Fn(&str) -> bool,
) -> PageRank<'a> {
    let nodes = graph.nodes().to_vec();
    if nodes.is_empty() {
        return PageRank {
            nodes,
            ranks: Vec::new(),
            support: Vec::new(),
            converged: true,
        };
    }
    let teleport = personalization(&nodes, seeds);
    let adjacency = walk_adjacency(graph, &is_pure_source);
    let support = reachable(&adjacency, &teleport);
    let totals: Vec<f64> = adjacency
        .iter()
        .map(|cells| compensated_sum(&weights(cells)))
        .collect();
    let (ranks, converged) = iterate(&adjacency, &totals, &teleport);
    PageRank {
        nodes,
        ranks,
        support,
        converged,
    }
}

/// Return the ranked paths, highest rank first, ties in path order, with the seeds leading in
/// that same order.
#[must_use]
pub fn rank_order<'a>(rank: &PageRank<'a>, is_seed: impl Fn(&str) -> bool) -> Vec<&'a str> {
    let mut ordered: Vec<(&'a str, f64)> = rank.ranks().collect();
    ordered.sort_by(|left, right| descending(left.1, right.1).then_with(|| left.0.cmp(right.0)));
    let (seeds, rest): (Vec<_>, Vec<_>) = ordered.into_iter().partition(|(path, _)| is_seed(path));
    seeds
        .into_iter()
        .chain(rest)
        .map(|(path, _)| path)
        .collect()
}

// Ranks are finite, so the comparison is total; -0.0 and 0.0 compare equal.
fn descending(left: f64, right: f64) -> Ordering {
    right.partial_cmp(&left).unwrap_or(Ordering::Equal)
}

type Cells = Vec<(usize, f64)>;

fn weights(cells: &Cells) -> Vec<f64> {
    cells.iter().map(|&(_, weight)| weight).collect()
}

// The seeds' shares normalised over the graph's files, or one uniform share.
fn personalization(nodes: &[&str], seeds: &BTreeMap<String, f64>) -> Vec<f64> {
    if !seeds.is_empty() {
        let weighted: Vec<f64> = nodes
            .iter()
            .map(|node| seeds.get(*node).copied().unwrap_or(0.0))
            .collect();
        let total = compensated_sum(&weighted);
        if total > 0.0 {
            return weighted.iter().map(|weight| weight / total).collect();
        }
    }
    vec![1.0 / count(nodes.len()); nodes.len()]
}

// A file count is far below 2^32, so the float is exact.
fn count(files: usize) -> f64 {
    f64::from(u32::try_from(files).expect("a scan holds fewer than 2^32 files"))
}

// Forward edges plus the backflow; each cell holds at most a forward and a backward term.
fn walk_adjacency(graph: &FileGraph<'_>, is_pure_source: &impl Fn(&str) -> bool) -> Vec<Cells> {
    let mut collected: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); graph.nodes().len()];
    for (source, target, weight) in graph.edges() {
        let from = graph.position(source).expect("an edge starts at a node");
        let to = graph.position(target).expect("an edge ends at a node");
        *collected[from].entry(to).or_insert(0.0) += weight;
        if !is_pure_source(source) {
            *collected[to].entry(from).or_insert(0.0) += weight * BACKFLOW_WEIGHT;
        }
    }
    collected
        .into_iter()
        .map(|cells| cells.into_iter().collect())
        .collect()
}

fn reachable(adjacency: &[Cells], teleport: &[f64]) -> Vec<bool> {
    let mut seen: Vec<bool> = teleport.iter().map(|share| *share > 0.0).collect();
    let mut frontier: Vec<usize> = (0..seen.len()).filter(|&at| seen[at]).collect();
    while let Some(node) = frontier.pop() {
        for &(target, _) in &adjacency[node] {
            if !seen[target] {
                seen[target] = true;
                frontier.push(target);
            }
        }
    }
    seen
}

fn iterate(adjacency: &[Cells], totals: &[f64], teleport: &[f64]) -> (Vec<f64>, bool) {
    let mut rank = vec![1.0 / count(teleport.len()); teleport.len()];
    for _ in 0..MAX_ITERATIONS {
        let updated = step(adjacency, totals, teleport, &rank);
        let moved: Vec<f64> = updated
            .iter()
            .zip(&rank)
            .map(|(new, old)| (new - old).abs())
            .collect();
        rank = updated;
        if compensated_sum(&moved) < EPSILON {
            return (rank, true);
        }
    }
    (rank, false)
}

// One power-iteration pass; a file with no outgoing weight hands its mass to the teleport vector.
fn step(adjacency: &[Cells], totals: &[f64], teleport: &[f64], rank: &[f64]) -> Vec<f64> {
    let mut incoming = vec![0.0; rank.len()];
    let mut dangling = 0.0;
    for (source, cells) in adjacency.iter().enumerate() {
        let (mass, total) = (rank[source], totals[source]);
        if total <= 0.0 {
            dangling += mass;
            continue;
        }
        for &(target, weight) in cells {
            incoming[target] += mass * weight / total;
        }
    }
    teleport
        .iter()
        .zip(&incoming)
        .map(|(share, arriving)| (1.0 - DAMPING) * share + DAMPING * (arriving + dangling * share))
        .collect()
}
