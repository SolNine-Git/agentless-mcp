use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value, json};
use text::testing::{cases, float};

use super::corpus::{Corpus, bits};
use crate::communities::{community_label, detect_communities};
use crate::files::{FileGraph, Weighting};
use crate::flood::flood_backward;
use crate::names::{NameIndex, common_name_damping, name_multiplier};
use crate::rank::{pagerank, rank_order};
use crate::scan::FileFacts;
use crate::scopes::Scopes;

const MANIFEST: &str = env!("CARGO_MANIFEST_DIR");

fn weightings() -> Vec<(String, BTreeSet<String>, bool)> {
    cases(MANIFEST, "units", "weightings")
}

// Each weighting's graph over one corpus, under its fixture name.
fn graphs<'a>(
    index: &NameIndex<'a>,
    scopes: &Scopes<'a>,
    stoplists: &'a [(String, BTreeSet<String>, bool)],
) -> BTreeMap<&'a str, FileGraph<'a>> {
    stoplists
        .iter()
        .map(|(name, stoplist, relation_weights)| {
            let weighting = Weighting {
                stoplist,
                relation_weights: *relation_weights,
            };
            let graph =
                FileGraph::build(index, scopes, &weighting, None).expect("no deadline stops it");
            (name.as_str(), graph)
        })
        .collect()
}

fn with_graphs(check: impl Fn(&Corpus, &BTreeMap<&str, FileGraph<'_>>)) {
    let stoplists = weightings();
    for corpus in Corpus::all() {
        let files: Vec<FileFacts> = corpus.files();
        let index = NameIndex::build(&files);
        let scopes = Scopes::build(&files);
        check(&corpus, &graphs(&index, &scopes, &stoplists));
    }
}

#[test]
fn file_graph_edges_match_the_reference_in_insertion_order() {
    with_graphs(|corpus, graphs| {
        let found: Vec<Value> = graphs
            .iter()
            .flat_map(|(name, graph)| {
                graph.edges().map(move |(source, target, weight)| {
                    json!([name, source, target, bits(weight)])
                })
            })
            .collect();
        let mut expected: Vec<Value> = corpus.cases("file_graph");
        // The fixture lists the weightings in table order, the map above in name order.
        expected.sort_by_key(|case| case[0].as_str().map(str::to_owned));
        assert_eq!(found, expected, "{}", corpus.name);
    });
}

#[derive(Deserialize)]
struct RankCase {
    weighting: String,
    seeds: Vec<(String, String)>,
    pure_sources: BTreeSet<String>,
    converged: bool,
    ranks: Vec<(String, String)>,
    support: Vec<String>,
    order: Vec<String>,
}

#[test]
fn ranks_match_the_reference() {
    with_graphs(|corpus, graphs| {
        for case in corpus.cases::<RankCase>("pagerank") {
            let graph = &graphs[case.weighting.as_str()];
            let seeds: BTreeMap<String, f64> = case
                .seeds
                .iter()
                .map(|(path, share)| (path.clone(), float(share)))
                .collect();
            let pure_source = |path: &str| case.pure_sources.contains(path);
            let rank = pagerank(graph, &seeds, pure_source, None).expect("no deadline stops it");
            let ranks: Vec<(String, String)> = rank
                .ranks()
                .map(|(path, value)| (path.to_owned(), bits(value)))
                .collect();
            let context = format!("{} {} {:?}", corpus.name, case.weighting, case.seeds);
            assert_eq!(ranks, case.ranks, "{context}");
            assert_eq!(rank.converged(), case.converged, "{context}");
            let support: Vec<&str> = graph
                .nodes()
                .iter()
                .copied()
                .filter(|path| rank.reached(path))
                .collect();
            assert_eq!(support, case.support, "{context}");
            assert_eq!(
                rank_order(&rank, |path| seeds.contains_key(path)),
                case.order,
                "{context}"
            );
        }
    });
}

#[derive(Deserialize)]
struct FloodCase {
    seeds: Vec<String>,
    max_depth: u32,
    reached: Vec<(String, u32)>,
    exhausted: bool,
}

#[test]
fn floods_match_the_reference() {
    with_graphs(|corpus, graphs| {
        for case in corpus.cases::<FloodCase>("flood") {
            let flood = flood_backward(
                &graphs["default"],
                case.seeds.iter().map(String::as_str),
                case.max_depth,
            );
            let reached: Vec<(String, u32)> = flood
                .reached
                .iter()
                .map(|row| (row.path.to_owned(), row.depth))
                .collect();
            assert_eq!(reached, case.reached, "{} {:?}", corpus.name, case.seeds);
            assert_eq!(
                flood.exhausted, case.exhausted,
                "{} {:?}",
                corpus.name, case.seeds
            );
        }
    });
}

#[derive(Deserialize)]
struct CommunityCase {
    resolution: String,
    communities: Value,
    modularity: String,
    standard_modularity: String,
    converged: bool,
}

#[test]
fn communities_match_the_reference() {
    with_graphs(|corpus, graphs| {
        for case in corpus.cases::<CommunityCase>("communities") {
            let resolution = float(&case.resolution);
            let partition = detect_communities(&graphs["default"], resolution, None)
                .expect("no deadline stops it");
            let found: Vec<Value> = partition
                .communities
                .iter()
                .map(|community| {
                    let weights = (
                        bits(community.internal_weight),
                        bits(community.total_weight),
                    );
                    json!([community.label, community.members, weights.0, weights.1])
                })
                .collect();
            let context = format!("{} {}", corpus.name, case.resolution);
            assert_eq!(Value::from(found), case.communities, "{context}");
            assert_eq!(bits(partition.modularity), case.modularity, "{context}");
            assert_eq!(
                bits(partition.standard_modularity),
                case.standard_modularity,
                "{context}"
            );
            assert_eq!(partition.converged, case.converged, "{context}");
        }
    });
}

#[test]
fn weight_rules_match_the_reference() {
    for (name, stoplist, expected) in
        cases::<Vec<(String, BTreeSet<String>, String)>>(MANIFEST, "units", "name_multiplier")
    {
        assert_eq!(
            bits(name_multiplier(&name, &stoplist)),
            expected,
            "{name:?} {stoplist:?}"
        );
    }
    for (spread, expected) in cases::<Vec<(u32, String)>>(MANIFEST, "units", "damping") {
        assert_eq!(bits(common_name_damping(spread)), expected, "{spread}");
    }
    for (members, expected) in
        cases::<Vec<(Vec<String>, String)>>(MANIFEST, "units", "community_label")
    {
        let members: Vec<&str> = members.iter().map(String::as_str).collect();
        assert_eq!(community_label(&members), expected, "{members:?}");
    }
}

// Two communities in one directory, a star and a pair, and a file with no edge.
#[test]
fn a_label_names_the_directory_and_the_hub_or_the_one_file() {
    let nodes = vec![
        "lone.py",
        "src/a.py",
        "src/b.py",
        "src/c.py",
        "src/hub.py",
        "src/x.py",
        "src/y.py",
    ];
    let edges = [
        ("src/a.py", "src/hub.py", 1.0),
        ("src/b.py", "src/hub.py", 1.0),
        ("src/c.py", "src/hub.py", 1.0),
        ("src/x.py", "src/y.py", 1.0),
    ];
    let graph = FileGraph::from_parts(nodes, &edges);
    let partition = detect_communities(&graph, 1.0, None).expect("no deadline stops it");
    let labels: Vec<&str> = (partition.communities.iter())
        .map(|community| community.label.as_str())
        .collect();
    assert_eq!(
        labels,
        ["src around src/hub.py", "src around src/x.py", "lone.py"]
    );
}

#[derive(Deserialize)]
struct FloodBound {
    leaves: usize,
    outers: usize,
    max_depth: u32,
    rows: usize,
    first: Vec<(String, u32)>,
    last: Vec<(String, u32)>,
    exhausted: bool,
}

#[test]
fn a_flood_past_its_visit_bound_matches_the_reference() {
    let case = cases::<Vec<FloodBound>>(MANIFEST, "units", "flood_bound").remove(0);
    let leaves: Vec<String> = (0..case.leaves)
        .map(|number| format!("leaf{number:05}.py"))
        .collect();
    let outers: Vec<String> = (0..case.outers)
        .map(|number| format!("outer{number:02}.py"))
        .collect();
    let mut nodes: Vec<&str> = leaves.iter().chain(&outers).map(String::as_str).collect();
    nodes.push("hub.py");
    nodes.sort_unstable();
    let mut edges: Vec<(&str, &str, f64)> = leaves
        .iter()
        .map(|leaf| (leaf.as_str(), "hub.py", 1.0))
        .collect();
    edges.extend(
        outers
            .iter()
            .zip(&leaves)
            .map(|(outer, leaf)| (outer.as_str(), leaf.as_str(), 1.0)),
    );
    let graph = FileGraph::from_parts(nodes, &edges);
    let flood = flood_backward(&graph, ["hub.py"], case.max_depth);
    let rows: Vec<(String, u32)> = flood
        .reached
        .iter()
        .map(|row| (row.path.to_owned(), row.depth))
        .collect();
    assert_eq!(rows.len(), case.rows);
    assert_eq!(rows[..case.first.len()], case.first);
    assert_eq!(rows[rows.len() - case.last.len()..], case.last);
    assert_eq!(flood.exhausted, case.exhausted);
}
