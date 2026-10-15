use std::collections::{BTreeMap, BTreeSet};

use facts::{Symbol, SymbolKind};
use serde_json::{Value, json};

use super::corpus::{Corpus, bits, edge, reference};
use crate::communities::detect_communities;
use crate::cycles::{Adjacency, components, import_cycles};
use crate::files::{FileGraph, Weighting};
use crate::flood::flood_backward;
use crate::names::NameIndex;
use crate::owners::{LineOwners, enclosing_symbol};
use crate::path::{DEFAULT_MAX_VISITED, PathPolicy, shortest_path};
use crate::rank::{pagerank, rank_order};
use crate::resolver::Resolver;
use crate::scopes::Scopes;
use crate::symbols::SymbolGraph;

const WEIGHTS: [f64; 6] = [0.0125, 0.025, 0.1, 0.25, 1.0, 3.0];
const SHARES: [f64; 4] = [0.5, 1.0 / 3.0, 1.0, 0.25];

// A xorshift generator: the same graphs on every run, with no dependency.
struct Random(u64);

impl Random {
    fn next(&mut self, below: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % u64::try_from(below).expect("a small bound"))
            .expect("a value below a usize bound")
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.next(100) < percent
    }
}

fn names(count: usize) -> Vec<String> {
    (0..count)
        .map(|number| format!("f{number:03}.py"))
        .collect()
}

// Distinct directed pairs with positive weights and no self loop, as the build emits them.
fn random_edges<'a>(random: &mut Random, nodes: &'a [String]) -> Vec<(&'a str, &'a str, f64)> {
    let density = 5 + random.next(40);
    let pairs = nodes
        .iter()
        .flat_map(|source| nodes.iter().map(move |target| (source, target)));
    let mut edges = Vec::new();
    for (source, target) in pairs.filter(|(source, target)| source != target) {
        if random.chance(density) {
            edges.push((
                source.as_str(),
                target.as_str(),
                WEIGHTS[random.next(WEIGHTS.len())],
            ));
        }
    }
    edges
}

fn random_graph<'a>(random: &mut Random, nodes: &'a [String]) -> FileGraph<'a> {
    let edges = random_edges(random, nodes);
    FileGraph::from_parts(nodes.iter().map(String::as_str).collect(), &edges)
}

fn random_seeds(random: &mut Random, nodes: &[String]) -> BTreeMap<String, f64> {
    let mut seeds = BTreeMap::new();
    for node in nodes {
        let share = SHARES[random.next(SHARES.len())];
        if random.chance(15) {
            seeds.insert(node.clone(), share);
        }
    }
    seeds
}

#[test]
fn ranks_sum_to_one() {
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    for _ in 0..300 {
        let nodes = names(1 + random.next(40));
        let graph = random_graph(&mut random, &nodes);
        let seeds = random_seeds(&mut random, &nodes);
        let pure: BTreeSet<&String> = nodes.iter().filter(|_| random.chance(10)).collect();
        for seeding in [BTreeMap::new(), seeds] {
            let rank = pagerank(&graph, &seeding, |path| {
                pure.iter().any(|held| *held == path)
            });
            let total: f64 = rank.ranks().map(|(_, value)| value).sum();
            assert!(
                (total - 1.0).abs() < 1e-9,
                "{total} over {} nodes",
                nodes.len()
            );
        }
    }
}

#[test]
fn a_partition_covers_every_node_exactly_once() {
    let mut random = Random(0x2545_f491_4f6c_dd1d);
    for _ in 0..300 {
        let nodes = names(1 + random.next(40));
        let graph = random_graph(&mut random, &nodes);
        for resolution in [0.25, 1.0, 4.0] {
            let partition = detect_communities(&graph, resolution);
            let mut members: Vec<&str> = partition
                .communities
                .iter()
                .flat_map(|community| community.members.iter().copied())
                .collect();
            members.sort_unstable();
            assert_eq!(members, graph.nodes(), "resolution {resolution}");
        }
    }
}

// Every public output of one corpus, as text: a hash order that reached any of them would
// differ between two builds, since each std map instance draws its own hasher seed.
fn outputs(corpus: &Corpus) -> Vec<String> {
    let files = corpus.files();
    let index = NameIndex::build(&files);
    let scopes = Scopes::build(&files);
    let stoplist = BTreeSet::from(["helper".to_owned(), "render".to_owned()]);
    let weighting = Weighting {
        stoplist: &stoplist,
        relation_weights: true,
    };
    let graph = FileGraph::build(&index, &scopes, &weighting);
    let edges: Vec<Value> = graph
        .edges()
        .map(|(source, target, weight)| json!([source, target, bits(weight)]))
        .collect();
    let seeds: BTreeMap<String, f64> = graph
        .nodes()
        .iter()
        .take(2)
        .map(|path| ((*path).to_owned(), 0.5))
        .collect();
    let rank = pagerank(&graph, &seeds, |path| path.contains("test"));
    let ranks: Vec<Value> = rank
        .ranks()
        .map(|(path, value)| json!([path, bits(value)]))
        .collect();
    let order = rank_order(&rank, |path| seeds.contains_key(path));
    let flood = flood_backward(&graph, graph.nodes().iter().copied(), 2);
    let partition = detect_communities(&graph, 1.0);
    let resolver = Resolver::new(&index, &scopes);
    let symbols = SymbolGraph::build(&resolver);
    let symbol_edges: Vec<Value> = symbols.edges().iter().map(edge).collect();
    let ambiguous: Vec<Value> = symbols.ambiguous().iter().map(reference).collect();
    let cycles = import_cycles(&SymbolGraph::imports(&scopes));
    let policy = PathPolicy {
        include_unique: true,
        include_ambiguous: true,
    };
    let paths: Vec<Value> = symbols
        .files()
        .iter()
        .zip(symbols.files().iter().rev())
        .map(|(source, target)| {
            let found = shortest_path(&symbols, source, target, policy, DEFAULT_MAX_VISITED);
            json!([
                found
                    .hops
                    .iter()
                    .map(|hop| edge(&hop.edge))
                    .collect::<Vec<_>>(),
                found.visited
            ])
        })
        .collect();
    vec![
        json!(edges).to_string(),
        json!(ranks).to_string(),
        format!("{order:?} {:?} {:?}", flood, partition),
        json!([symbol_edges, ambiguous, paths]).to_string(),
        format!("{cycles:?}"),
    ]
}

#[test]
fn every_output_built_twice_is_identical() {
    for corpus in Corpus::all() {
        assert_eq!(outputs(&corpus), outputs(&corpus), "{}", corpus.name);
    }
}

fn symbol(line: u32, end_line: u32) -> Symbol {
    Symbol {
        name: format!("s{line}_{end_line}"),
        kind: SymbolKind::Function,
        line,
        end_line,
        signature: String::new(),
        parent: String::new(),
        decorated: false,
        bases: Vec::new(),
        is_public: true,
        rationales: Vec::new(),
        duplicate_index: 0,
    }
}

#[test]
fn line_owners_agree_with_enclosing_symbol() {
    let mut random = Random(0x1234_5678_9abc_def1);
    for _ in 0..2_000 {
        let symbols: Vec<Symbol> = (0..random.next(12))
            .map(|_| {
                let line = 1 + u32::try_from(random.next(40)).expect("a small line");
                let span = u32::try_from(random.next(16)).expect("a small span");
                // Now and then a span that ends before it starts, which owns no line.
                if random.chance(5) {
                    symbol(line, line - 1)
                } else {
                    symbol(line, line + span)
                }
            })
            .collect();
        let owners = LineOwners::build(&symbols);
        for line in 0..70 {
            let direct = enclosing_symbol(&symbols, line).map(std::ptr::from_ref);
            let tabled = owners.owner(line).map(std::ptr::from_ref);
            assert_eq!(direct, tabled, "line {line}");
        }
    }
}

fn reaches(adjacency: &Adjacency<'_>, from: &str, to: &str) -> bool {
    let mut seen = BTreeSet::from([from]);
    let mut frontier = vec![from];
    while let Some(node) = frontier.pop() {
        for &next in adjacency.get(node).map_or(&[][..], Vec::as_slice) {
            if next == to {
                return true;
            }
            if seen.insert(next) {
                frontier.push(next);
            }
        }
    }
    false
}

#[test]
fn tarjan_components_are_the_mutual_reachability_classes() {
    let mut random = Random(0x0f1e_2d3c_4b5a_6978);
    for _ in 0..300 {
        let owned = names(1 + random.next(14));
        let nodes: Vec<&str> = owned.iter().map(String::as_str).collect();
        let mut adjacency: Adjacency<'_> = BTreeMap::new();
        for (source, target, _) in random_edges(&mut random, &owned) {
            adjacency.entry(source).or_default().push(target);
        }
        let found: BTreeSet<Vec<&str>> = components(&nodes, &adjacency).into_iter().collect();
        let expected: BTreeSet<Vec<&str>> = nodes
            .iter()
            .map(|&node| {
                let mutual = |other: &&str| {
                    *other == node
                        || (reaches(&adjacency, node, other) && reaches(&adjacency, other, node))
                };
                nodes.iter().copied().filter(mutual).collect()
            })
            .collect();
        assert_eq!(found, expected);
    }
}
