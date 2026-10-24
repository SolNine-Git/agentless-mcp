use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use kernel::testing::PinnedClock;
use kernel::{Deadline, Stopped};

use super::corpus::Corpus;
use crate::{
    DEFAULT_MAX_VISITED, FileFacts, FileGraph, NameIndex, PathBounds, PathPolicy, Resolver, Scopes,
    SymbolGraph, Weighting, detect_communities, pagerank, shortest_path,
};

// A deadline whose allowance ran out a minute ago.
fn spent() -> Deadline {
    let clock = Arc::new(PinnedClock::at(UNIX_EPOCH));
    let deadline = Deadline::new(1.0, 0.0, clock.clone());
    clock.advance(Duration::from_secs(61));
    deadline
}

fn is_spent<T>(result: &Result<T, Stopped>) -> bool {
    matches!(result, Err(Stopped::Spent { .. }))
}

// The repo_py corpus: three files with references, imports and file-graph edges.
fn files() -> Vec<FileFacts> {
    Corpus::load("repo_py").files()
}

fn file_graph<'a>(
    index: &NameIndex<'a>,
    scopes: &Scopes<'a>,
    deadline: Option<&Deadline>,
) -> Result<FileGraph<'a>, Stopped> {
    let stoplist = BTreeSet::new();
    let weighting = Weighting {
        stoplist: &stoplist,
        relation_weights: false,
    };
    FileGraph::build(index, scopes, &weighting, deadline)
}

#[test]
fn the_file_graph_stops_under_a_spent_deadline() {
    let files = files();
    let (index, scopes) = (NameIndex::build(&files), Scopes::build(&files));
    assert!(is_spent(&file_graph(&index, &scopes, Some(&spent()))));
}

#[test]
fn the_ranking_stops_under_a_spent_deadline() {
    let files = files();
    let (index, scopes) = (NameIndex::build(&files), Scopes::build(&files));
    let graph = file_graph(&index, &scopes, None).expect("no deadline stops it");
    let ranked = pagerank(&graph, &BTreeMap::new(), |_| false, Some(&spent()));
    assert!(is_spent(&ranked));
}

#[test]
fn the_community_passes_stop_under_a_spent_deadline() {
    let files = files();
    let (index, scopes) = (NameIndex::build(&files), Scopes::build(&files));
    let graph = file_graph(&index, &scopes, None).expect("no deadline stops it");
    assert!(graph.edge_count() > 0, "a graph with no edges runs no pass");
    assert!(is_spent(&detect_communities(&graph, 1.0, Some(&spent()))));
}

#[test]
fn reference_resolution_stops_under_a_spent_deadline() {
    let files = files();
    let (index, scopes) = (NameIndex::build(&files), Scopes::build(&files));
    let resolver = Resolver::new(&index, &scopes);
    assert!(is_spent(&SymbolGraph::build(&resolver, Some(&spent()))));
}

#[test]
fn the_path_search_stops_under_a_spent_deadline() {
    let files = files();
    let (index, scopes) = (NameIndex::build(&files), Scopes::build(&files));
    let resolver = Resolver::new(&index, &scopes);
    let graph = SymbolGraph::build(&resolver, None).expect("no deadline stops it");
    let deadline = spent();
    let bounds = PathBounds {
        max_visited: DEFAULT_MAX_VISITED,
        deadline: Some(&deadline),
    };
    let policy = PathPolicy::default();
    let found = shortest_path(&graph, "reports.py", "pricing.py", policy, bounds);
    assert!(is_spent(&found));
}

#[test]
fn a_cancelled_deadline_stops_the_ranking_with_its_own_reason() {
    let files = files();
    let (index, scopes) = (NameIndex::build(&files), Scopes::build(&files));
    let graph = file_graph(&index, &scopes, None).expect("no deadline stops it");
    let deadline = Deadline::new(60.0, 0.0, Arc::new(PinnedClock::at(UNIX_EPOCH)));
    deadline.cancel();
    let ranked = pagerank(&graph, &BTreeMap::new(), |_| false, Some(&deadline));
    assert_eq!(ranked, Err(Stopped::Cancelled));
}
