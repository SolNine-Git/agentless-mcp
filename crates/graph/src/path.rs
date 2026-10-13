//! The fewest-hop path between two nodes of the symbol graph, edges walked both ways.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::resolver::Tier;
use crate::symbols::{Endpoint, Relation, SymbolEdge, SymbolGraph};

/// The nodes a path search looks at before it reports that it stopped looking.
pub const DEFAULT_MAX_VISITED: usize = 20_000;

/// Which non-binding evidence tiers a path may walk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PathPolicy {
    pub include_unique: bool,
    pub include_ambiguous: bool,
}

/// One step along a path, walked with the edge or against it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop<'a> {
    pub edge: SymbolEdge<'a>,
    pub forward: bool,
}

impl<'a> Hop<'a> {
    /// Return the endpoint the hop starts from.
    #[must_use]
    pub fn origin(&self) -> &Endpoint<'a> {
        if self.forward {
            self.edge.source()
        } else {
            self.edge.target()
        }
    }

    /// Return the endpoint the hop reaches.
    #[must_use]
    pub fn arrival(&self) -> &Endpoint<'a> {
        if self.forward {
            self.edge.target()
        } else {
            self.edge.source()
        }
    }
}

/// The outcome of one path search: the hops, or why there are none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathResult<'a> {
    pub hops: Vec<Hop<'a>>,
    pub found: bool,
    pub visited: usize,
    pub exhausted: bool,
}

/// Find the fewest-hop path from `source` to `target`, breadth first, each node's hops in
/// sort-key order; `max_visited` bounds the nodes expanded.
#[must_use]
pub fn shortest_path<'a>(
    graph: &SymbolGraph<'a>,
    source: &str,
    target: &str,
    policy: PathPolicy,
    max_visited: usize,
) -> PathResult<'a> {
    if source == target {
        return PathResult {
            hops: Vec::new(),
            found: true,
            visited: 1,
            exhausted: false,
        };
    }
    let mut search = Search::new(graph, policy, source);
    search.run(target, max_visited)
}

type Previous<'a> = BTreeMap<Arc<str>, (Arc<str>, Hop<'a>)>;

enum Level {
    Found,
    Exhausted,
    Next(Vec<Arc<str>>),
}

struct Search<'g, 'a> {
    graph: &'g SymbolGraph<'a>,
    policy: PathPolicy,
    adjacency: BTreeMap<&'g str, Vec<(usize, bool)>>,
    source: Arc<str>,
    previous: Previous<'a>,
    seen: BTreeSet<Arc<str>>,
    // A name expanded once has marked every node it reaches seen, so it is not expanded again.
    expanded: BTreeSet<(bool, &'a str)>,
    visited: usize,
}

impl<'g, 'a> Search<'g, 'a> {
    fn new(graph: &'g SymbolGraph<'a>, policy: PathPolicy, source: &str) -> Search<'g, 'a> {
        let source: Arc<str> = Arc::from(source);
        Search {
            graph,
            policy,
            adjacency: undirected(graph, policy),
            seen: BTreeSet::from([Arc::clone(&source)]),
            source,
            previous: BTreeMap::new(),
            expanded: BTreeSet::new(),
            visited: 0,
        }
    }

    fn run(&mut self, target: &str, max_visited: usize) -> PathResult<'a> {
        let mut frontier = vec![Arc::clone(&self.source)];
        while !frontier.is_empty() {
            match self.level(&frontier, target, max_visited) {
                Level::Found => {
                    let hops = unwind(&self.previous, &self.source, target);
                    return self.result(hops, true, false);
                }
                Level::Exhausted => return self.result(Vec::new(), false, true),
                Level::Next(following) => frontier = following,
            }
        }
        self.result(Vec::new(), false, false)
    }

    fn level(&mut self, frontier: &[Arc<str>], target: &str, max_visited: usize) -> Level {
        let mut following = Vec::new();
        for node in frontier {
            self.visited += 1;
            if self.visited > max_visited {
                return Level::Exhausted;
            }
            if self.expand(node, target, &mut following) {
                return Level::Found;
            }
        }
        Level::Next(following)
    }

    // Marks each new neighbour seen; returns true when `target` is reached.
    fn expand(&mut self, node: &Arc<str>, target: &str, following: &mut Vec<Arc<str>>) -> bool {
        for hop in self.hops_at(node) {
            let reached = Arc::clone(hop.arrival().node_arc());
            if !self.seen.insert(Arc::clone(&reached)) {
                continue;
            }
            self.previous
                .insert(Arc::clone(&reached), (Arc::clone(node), hop));
            if &*reached == target {
                return true;
            }
            following.push(reached);
        }
        false
    }

    fn hops_at(&mut self, node: &str) -> Vec<Hop<'a>> {
        let edges = &self.graph.edges;
        let mut hops: Vec<Hop<'a>> = self.adjacency.get(node).map_or_else(Vec::new, |list| {
            list.iter()
                .map(|&(at, forward)| Hop {
                    edge: edges[at].clone(),
                    forward,
                })
                .collect()
        });
        if self.policy.include_ambiguous {
            hops.extend(self.ambiguous_hops(node));
            hops.sort_by(|left, right| left.edge.cmp_sort_key(&right.edge));
        }
        hops
    }

    // Every arriving ambiguous edge, once a name is new here; and per (relation, name) only
    // the lowest-line reference, as the search takes the first hop to each neighbour.
    fn ambiguous_hops(&mut self, node: &str) -> Vec<Hop<'a>> {
        let graph = self.graph;
        let arriving: BTreeSet<(bool, &'a str)> = graph
            .candidates_at(node)
            .iter()
            .map(|(name, _)| (false, *name))
            .filter(|key| !self.expanded.contains(key))
            .collect();
        let mut hops: Vec<Hop<'a>> = Vec::new();
        if !arriving.is_empty() {
            let edges = graph.ambiguous_into(node);
            hops.extend(edges.map(|edge| Hop {
                edge,
                forward: false,
            }));
        }
        let mut first: Vec<(Relation, &'a str, usize)> = Vec::new();
        for (at, reference) in graph.references_from(node).iter().enumerate() {
            let key = (reference.relation(), reference.name());
            let fresh = !self.expanded.contains(&(true, reference.name()));
            if fresh
                && !first
                    .iter()
                    .any(|&(relation, name, _)| (relation, name) == key)
            {
                first.push((key.0, key.1, at));
            }
        }
        let references = graph.references_from(node);
        for &(_, name, at) in &first {
            let edges = graph.ambiguous_edges(&references[at]);
            hops.extend(edges.map(|edge| Hop {
                edge,
                forward: true,
            }));
            self.expanded.insert((true, name));
        }
        self.expanded.extend(arriving);
        hops
    }

    fn result(&self, hops: Vec<Hop<'a>>, found: bool, exhausted: bool) -> PathResult<'a> {
        PathResult {
            hops,
            found,
            visited: self.visited,
            exhausted,
        }
    }
}

// Each usable edge, walked forward from its source and backward from its target.
fn undirected<'g>(
    graph: &'g SymbolGraph<'_>,
    policy: PathPolicy,
) -> BTreeMap<&'g str, Vec<(usize, bool)>> {
    let mut adjacency: BTreeMap<&'g str, Vec<(usize, bool)>> = BTreeMap::new();
    for (at, edge) in graph.edges.iter().enumerate() {
        if edge.tier() == Tier::Unique && !policy.include_unique {
            continue;
        }
        adjacency
            .entry(edge.source().node())
            .or_default()
            .push((at, true));
        adjacency
            .entry(edge.target().node())
            .or_default()
            .push((at, false));
    }
    adjacency
}

fn unwind<'a>(previous: &Previous<'a>, source: &str, target: &str) -> Vec<Hop<'a>> {
    let mut hops = Vec::new();
    let mut node: &str = target;
    while node != source {
        let (parent, hop) = &previous[node];
        hops.push(hop.clone());
        node = parent;
    }
    hops.reverse();
    hops
}
