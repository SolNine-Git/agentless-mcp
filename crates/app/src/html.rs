//! The data of the interactive HTML export: the modules the ranking keeps, their communities,
//! and the edges between them, imports first, with an exact account of each bound's cut.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use graph::{CommunityPartition, detect_communities};

use crate::bounds::within;
use crate::file_graph::{Ranked, resolution_setting, select};
use crate::ops::limits::{MAX_HTML_EDGES, MAX_HTML_NODES};
use crate::scan::over_scan;
use crate::{Error, IntArg, Noted, Reading, Services};

/// An HTML export request: the node and edge bounds, and the resolution of the communities.
#[derive(Clone, Debug, PartialEq)]
pub struct HtmlRequest {
    /// The most modules the export holds.
    pub max_nodes: IntArg,
    /// The most edges the export holds.
    pub max_edges: IntArg,
    /// The modularity resolution of the community colours, or `None` for the default.
    pub resolution: Option<f64>,
}

/// The modules, edges and communities of one export, with what each bound left out.
#[derive(Clone, Debug, PartialEq)]
pub struct HtmlGraph {
    /// The modules, in path order.
    pub nodes: Vec<HtmlNode>,
    /// The edges, imports first, then by weight, highest first.
    pub edges: Vec<HtmlEdge>,
    /// The position and label of each community that holds a module, by position.
    pub communities: Vec<(usize, String)>,
    /// The node bound.
    pub node_bound: usize,
    /// The edge bound.
    pub edge_bound: usize,
    /// The modules the node bound left out.
    pub elided_nodes: usize,
    /// The edges that lost an endpoint to the node bound, or that join a module to itself.
    pub edges_without_both_nodes: usize,
    /// The edges the edge bound left out.
    pub edges_over_bound: usize,
}

/// One module of the export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlNode {
    /// The generated id, such as `n3`.
    pub id: String,
    /// The path below the repository root.
    pub path: String,
    /// The position of its community.
    pub community: usize,
}

/// One edge of the export.
#[derive(Clone, Debug, PartialEq)]
pub struct HtmlEdge {
    /// The source module's id.
    pub source: String,
    /// The target module's id.
    pub target: String,
    /// True for a declared import, false for a name reference.
    pub import: bool,
    /// The edge weight.
    pub weight: f64,
}

/// Builds the export data of the repository that `reading` names. The scan's warnings come with
/// either outcome.
#[must_use]
pub fn html(
    services: &Services,
    reading: &Reading<'_>,
    request: &HtmlRequest,
) -> Noted<Result<HtmlGraph, Error>> {
    let bounds = within(&request.max_nodes, 1, MAX_HTML_NODES, "max_nodes").and_then(|nodes| {
        let edges = within(&request.max_edges, 0, MAX_HTML_EDGES, "max_edges")?;
        Ok((nodes, edges))
    });
    let (max_nodes, max_edges) = match bounds {
        Ok(bounds) => bounds,
        Err(error) => return Noted::new(Err(error)),
    };
    let config = &reading.repo.config;
    over_scan(services, reading, |facts| {
        let ranked = Ranked::build(&facts.files, config);
        let resolution = resolution_setting(request.resolution)?;
        let partition = detect_communities(&ranked.graph, resolution);
        Ok(export(&ranked, &partition, max_nodes, max_edges))
    })
}

fn export(
    ranked: &Ranked<'_>,
    partition: &CommunityPartition<'_>,
    max_nodes: usize,
    max_edges: usize,
) -> HtmlGraph {
    let selected = select(&ranked.graph, &ranked.ranking, max_nodes, None).selected;
    let membership = partition.index_of();
    let community = |path: &str| {
        let found = membership.get(path).copied();
        found.expect("a partition covers every module of the graph it was detected on")
    };
    let nodes: Vec<HtmlNode> = (selected.iter().enumerate())
        .map(|(position, path)| HtmlNode {
            id: format!("n{position}"),
            path: (*path).to_owned(),
            community: community(path),
        })
        .collect();
    let mut candidates = candidate_edges(ranked, &selected);
    let without_both_nodes = ranked.graph.edge_count() - candidates.len();
    let over_bound = candidates.len().saturating_sub(max_edges);
    candidates.truncate(max_edges);
    let represented: BTreeSet<usize> = nodes.iter().map(|node| node.community).collect();
    HtmlGraph {
        communities: (represented.into_iter())
            .map(|at| (at, partition.communities[at].label.clone()))
            .collect(),
        node_bound: max_nodes,
        edge_bound: max_edges,
        elided_nodes: ranked.graph.nodes().len() - nodes.len(),
        edges_without_both_nodes: without_both_nodes,
        edges_over_bound: over_bound,
        nodes,
        edges: candidates,
    }
}

// The edges between two kept modules, imports first, then by weight, then by id text. A module's
// edge to itself would draw a zero-length line and spend a slot of the bound.
fn candidate_edges(ranked: &Ranked<'_>, selected: &[&str]) -> Vec<HtmlEdge> {
    let id = |path: &str| {
        let at = selected.binary_search(&path).ok()?;
        Some(format!("n{at}"))
    };
    let mut edges: Vec<HtmlEdge> = (ranked.graph.edges())
        .filter(|(source, target, _)| source != target)
        .filter_map(|(source, target, weight)| {
            Some(HtmlEdge {
                source: id(source)?,
                target: id(target)?,
                import: ranked.imports.contains(&(source, target)),
                weight,
            })
        })
        .collect();
    edges.sort_by(|left, right| {
        (right.import.cmp(&left.import))
            .then_with(|| {
                right
                    .weight
                    .partial_cmp(&left.weight)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.target.cmp(&right.target))
    });
    edges
}

#[cfg(test)]
mod tests;
