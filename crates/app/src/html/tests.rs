use super::{HtmlGraph, HtmlRequest, html};
use crate::IntArg;
use crate::endpoints::scanned::{LINKED, Scanned};

fn exported(max_nodes: usize, max_edges: usize) -> HtmlGraph {
    let scanned = Scanned::new(&LINKED);
    let request = HtmlRequest {
        max_nodes: IntArg::from_usize(max_nodes),
        max_edges: IntArg::from_usize(max_edges),
        resolution: None,
    };
    let outcome = html(&scanned.services, &scanned.reading(), &request);
    outcome.value.unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn imports_come_first_and_every_community_drawn_is_named() {
    let graph = exported(200, 600);
    assert_eq!(graph.nodes.len(), 4);
    assert!(graph.edges.first().is_some_and(|edge| edge.import));
    let kinds: Vec<bool> = graph.edges.iter().map(|edge| edge.import).collect();
    assert!(kinds.windows(2).all(|pair| pair[0] >= pair[1]));
    for node in &graph.nodes {
        assert!(
            graph
                .communities
                .iter()
                .any(|(at, _)| *at == node.community)
        );
    }
}

#[test]
fn each_bound_counts_its_own_cut() {
    let whole = exported(200, 600);
    let cut = exported(200, 1);
    assert_eq!(cut.edges.len(), 1);
    assert_eq!(cut.edges_over_bound, whole.edges.len() - 1);
    assert_eq!(cut.edges_without_both_nodes, whole.edges_without_both_nodes);
    let fewer = exported(1, 600);
    assert_eq!((fewer.nodes.len(), fewer.elided_nodes), (1, 3));
    assert_eq!(fewer.edges_over_bound, 0);
}
