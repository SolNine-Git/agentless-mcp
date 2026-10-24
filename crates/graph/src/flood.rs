//! What reaches a set of files: a breadth-first walk against the file graph's edges.

use std::collections::{BTreeMap, BTreeSet};

use crate::files::FileGraph;

// The nodes a flood expands before it stops answering the question it was asked.
const MAX_FLOOD_VISITED: usize = 20_000;

/// One file a flood arrived at, and the fewest hops that arrive there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reached<'a> {
    pub path: &'a str,
    pub depth: u32,
}

/// The outcome of one flood: what it reached, and whether it stopped at its visit bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flood<'a> {
    pub reached: Vec<Reached<'a>>,
    pub exhausted: bool,
}

/// Walk from `seeds` against the edges, at most `max_depth` hops, and report every file that
/// reaches a seed, ordered by depth, then path; the seeds themselves are never reported.
#[must_use]
pub fn flood_backward<'a, 's>(
    graph: &FileGraph<'a>,
    seeds: impl IntoIterator<Item = &'s str>,
    max_depth: u32,
) -> Flood<'a> {
    let frontier: BTreeSet<&'a str> = seeds
        .into_iter()
        .filter_map(|seed| graph.position(seed).map(|at| graph.nodes()[at]))
        .collect();
    if frontier.is_empty() {
        return Flood {
            reached: Vec::new(),
            exhausted: false,
        };
    }
    let mut walk = Walk {
        incoming: incoming(graph),
        seen: frontier.clone(),
        depths: BTreeMap::new(),
        visited: 0,
    };
    let mut frontier: Vec<&'a str> = frontier.into_iter().collect();
    let mut depth = 0;
    let mut exhausted = false;
    while !frontier.is_empty() && depth < max_depth && !exhausted {
        depth += 1;
        (frontier, exhausted) = walk.level(&frontier, depth);
    }
    let mut reached: Vec<Reached<'a>> = walk
        .depths
        .into_iter()
        .map(|(path, depth)| Reached { path, depth })
        .collect();
    reached.sort_by_key(|row| (row.depth, row.path));
    Flood { reached, exhausted }
}

struct Walk<'a> {
    incoming: BTreeMap<&'a str, Vec<&'a str>>,
    seen: BTreeSet<&'a str>,
    depths: BTreeMap<&'a str, u32>,
    visited: usize,
}

impl<'a> Walk<'a> {
    // Expand one level; returns the next frontier, in discovery order, and whether the visit
    // bound stopped the level.
    fn level(&mut self, frontier: &[&'a str], depth: u32) -> (Vec<&'a str>, bool) {
        let mut following = Vec::new();
        for node in frontier {
            self.visited += 1;
            if self.visited > MAX_FLOOD_VISITED {
                return (following, true);
            }
            let referrers = self.incoming.get(node).map_or(&[][..], Vec::as_slice);
            let fresh: Vec<&'a str> = referrers
                .iter()
                .copied()
                .filter(|other| self.seen.insert(other))
                .collect();
            self.depths
                .extend(fresh.iter().map(|other| (*other, depth)));
            following.extend(fresh);
        }
        (following, false)
    }
}

// Each file's referrers, in path order.
fn incoming<'a>(graph: &FileGraph<'a>) -> BTreeMap<&'a str, Vec<&'a str>> {
    let mut collected: BTreeMap<&'a str, Vec<&'a str>> = BTreeMap::new();
    for (source, target, _) in graph.edges() {
        collected.entry(target).or_default().push(source);
    }
    for sources in collected.values_mut() {
        sources.sort_unstable();
    }
    collected
}
