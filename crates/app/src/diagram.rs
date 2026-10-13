//! The module graph as a flowchart: the modules the ranking keeps, optionally one module's
//! neighbourhood and community subgraphs, declared imports apart from name references, and
//! labels reduced to an allowlist so that repository text stays inert.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use graph::{
    CommunityPartition, FileFacts, FileGraph, NameIndex, PageRank, Scopes, detect_communities,
    pagerank,
};
use io::ProjectConfig;
use text::strip;

use crate::bounds::within;
use crate::communities::{file_graph, over_scan, resolution_setting};
use crate::map::{Known, focus_resolution};
use crate::ops::limits::{MAX_DIAGRAM_EDGES, MAX_DIAGRAM_NODES};
use crate::{Error, IntArg, Noted, Reading, Services, is_test_path};

// Two hops is what "what touches this" means: what it calls, and what those call.
const FOCUS_DISTANCE: usize = 2;
const LABEL_MAX_CHARS: usize = 60;
const REPLACEMENT: char = '_';
const UNNAMED_LABEL: &str = "unnamed";

/// A diagram request: the focus module, the node and edge bounds, and the grouping.
#[derive(Clone, Debug, PartialEq)]
pub struct DiagramRequest {
    /// A module, resolved as map --focus resolves it, whose neighbourhood alone is drawn.
    pub focus: Option<String>,
    /// The most modules drawn.
    pub max_nodes: IntArg,
    /// The most edges before the reference edges are left out.
    pub max_edges: IntArg,
    /// Whether the modules are drawn inside community subgraphs.
    pub group_by_communities: bool,
    /// The modularity resolution of the grouping, or `None` for the default.
    pub resolution: Option<f64>,
}

/// A flowchart and an account of what its bounds left out, or the message that there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagramView {
    /// The flowchart, or `None` when the focus resolved to no module.
    pub chart: Option<Flowchart>,
    /// The modules drawn.
    pub nodes: usize,
    /// The candidate modules the node bound left out.
    pub elided: usize,
    /// Whether the modules are drawn inside community subgraphs.
    pub grouped: bool,
    /// The focus module, or the text the caller gave when it resolved to none.
    pub focus: String,
    /// Why there is no flowchart, or empty.
    pub message: String,
    /// The reference edges the edge bound left out.
    pub edges_over_bound: usize,
    /// Whether the ranking that chose the modules converged.
    pub rank_converged: bool,
}

/// The parts of a flowchart, each with a generated id and an inert label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flowchart {
    /// The community subgraphs that hold a drawn module, in partition order.
    pub groups: Vec<ChartGroup>,
    /// The drawn modules in no subgraph, in path order.
    pub nodes: Vec<ChartNode>,
    /// The edges drawn, in id order.
    pub edges: Vec<ChartEdge>,
    /// The edge bound the reference edges were held to.
    pub edge_bound: usize,
}

/// One drawn module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChartNode {
    /// The generated id, such as `n3`.
    pub id: String,
    /// The label.
    pub label: String,
}

/// One community subgraph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChartGroup {
    /// The generated id, such as `s0`.
    pub id: String,
    /// The title, which names the whole community.
    pub title: String,
    /// The drawn members, in path order.
    pub members: Vec<ChartNode>,
}

/// One drawn edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChartEdge {
    /// The source module's id.
    pub from: String,
    /// The target module's id.
    pub to: String,
    /// True for a declared import, false for a name reference.
    pub solid: bool,
}

/// Draws the module graph of the repository that `reading` names. The scan's warnings come with
/// either outcome.
#[must_use]
pub fn diagram(
    services: &Services,
    reading: &Reading<'_>,
    request: &DiagramRequest,
) -> Noted<Result<DiagramView, Error>> {
    let bounds = within(&request.max_nodes, 1, MAX_DIAGRAM_NODES, "max_nodes").and_then(|nodes| {
        let edges = within(&request.max_edges, 0, MAX_DIAGRAM_EDGES, "max_edges")?;
        Ok((nodes, edges))
    });
    let (max_nodes, max_edges) = match bounds {
        Ok(bounds) => bounds,
        Err(error) => return Noted::new(Err(error)),
    };
    let config = &reading.repo.config;
    over_scan(services, reading, |facts| {
        let ranked = Ranked::build(&facts.files, config);
        let focus = match diagram_focus(request.focus.as_deref(), &ranked) {
            Ok(focus) => focus,
            Err(message) => return Ok(empty(request, message)),
        };
        // Checked whether or not the caller groups, so a mistyped knob is always heard.
        let resolution = resolution_setting(request.resolution)?;
        let partition =
            (request.group_by_communities).then(|| detect_communities(&ranked.graph, resolution));
        let bounds = Bounds {
            max_nodes,
            max_edges,
            focus,
        };
        Ok(drawn(&ranked, partition.as_ref(), &bounds))
    })
}

// One call's file graph, its ranking and the file pairs a declared import connects.
pub(crate) struct Ranked<'a> {
    index: NameIndex<'a>,
    pub(crate) graph: FileGraph<'a>,
    pub(crate) ranking: PageRank<'a>,
    pub(crate) imports: BTreeSet<(&'a str, &'a str)>,
}

impl<'a> Ranked<'a> {
    // Tests stay pure sources of rank, as they do in the map.
    pub(crate) fn build(files: &'a [FileFacts], config: &ProjectConfig) -> Ranked<'a> {
        let index = NameIndex::build(files);
        let scopes = Scopes::build(files);
        let graph = file_graph(&index, &scopes, config);
        let ranking = pagerank(&graph, &BTreeMap::new(), is_test_path);
        Ranked {
            imports: scopes.import_pairs(),
            index,
            graph,
            ranking,
        }
    }
}

// The bounds of one flowchart and its focus module.
struct Bounds {
    max_nodes: usize,
    max_edges: usize,
    focus: Option<String>,
}

// The focus module, or the message that the text names none or several. A diagram has one
// centre, so several matches are answered with the list.
fn diagram_focus(focus: Option<&str>, ranked: &Ranked<'_>) -> Result<Option<String>, String> {
    let entry = focus.map(strip).unwrap_or_default();
    if entry.is_empty() {
        return Ok(None);
    }
    let known: Known<'_> = ranked.graph.nodes().iter().copied().collect();
    let (mut matches, _) = focus_resolution(entry, &known, &ranked.index);
    match matches.len() {
        0 => Err(format!("no module matches {entry}")),
        1 => Ok(matches.pop()),
        _ => Err(format!(
            "{entry} matches several modules: {}",
            matches.join(", ")
        )),
    }
}

fn empty(request: &DiagramRequest, message: String) -> DiagramView {
    DiagramView {
        chart: None,
        nodes: 0,
        elided: 0,
        grouped: request.group_by_communities,
        focus: request.focus.clone().unwrap_or_default(),
        message,
        edges_over_bound: 0,
        rank_converged: true,
    }
}

fn drawn(
    ranked: &Ranked<'_>,
    partition: Option<&CommunityPartition<'_>>,
    bounds: &Bounds,
) -> DiagramView {
    let focus = bounds.focus.as_deref();
    let selection = select(&ranked.graph, &ranked.ranking, bounds.max_nodes, focus);
    let ids: BTreeMap<&str, usize> = (selection.selected.iter().enumerate())
        .map(|(index, node)| (*node, index))
        .collect();
    let labels = distinct(selection.selected.iter().map(|node| safe_label(node)));
    let (groups, nodes) = chart_nodes(&selection.selected, &labels, partition);
    let (edges, over_bound) = chart_edges(&ranked.graph, &ids, &ranked.imports, bounds.max_edges);
    DiagramView {
        chart: Some(Flowchart {
            groups,
            nodes,
            edges,
            edge_bound: bounds.max_edges,
        }),
        nodes: selection.selected.len(),
        elided: selection.candidates - selection.selected.len(),
        grouped: partition.is_some(),
        focus: focus.unwrap_or_default().to_owned(),
        message: String::new(),
        edges_over_bound: over_bound,
        rank_converged: ranked.ranking.converged(),
    }
}

// The modules a flowchart draws, in path order, and how many candidates the bound chose from.
pub(crate) struct Selection<'a> {
    pub(crate) selected: Vec<&'a str>,
    pub(crate) candidates: usize,
}

// The highest-ranked candidates, the focus module always among them.
pub(crate) fn select<'a>(
    graph: &FileGraph<'a>,
    ranking: &PageRank<'a>,
    max_nodes: usize,
    focus: Option<&str>,
) -> Selection<'a> {
    let candidates = match focus {
        Some(seed) => neighbourhood(graph, seed),
        None => graph.nodes().iter().copied().collect(),
    };
    let rank = |node: &str| ranking.rank(node).unwrap_or(0.0);
    let mut ordered: Vec<&'a str> = candidates.iter().copied().collect();
    ordered
        .sort_by(|left, right| descending(rank(left), rank(right)).then_with(|| left.cmp(right)));
    if let Some(at) = focus.and_then(|seed| ordered.iter().position(|node| *node == seed)) {
        let seed = ordered.remove(at);
        ordered.insert(0, seed);
    }
    ordered.truncate(max_nodes);
    ordered.sort_unstable();
    Selection {
        selected: ordered,
        candidates: candidates.len(),
    }
}

// Ranks are finite; -0.0 and 0.0 compare equal, as the reference's tuple comparison has them.
fn descending(left: f64, right: f64) -> Ordering {
    right.partial_cmp(&left).unwrap_or(Ordering::Equal)
}

// The modules within two undirected hops of `seed`: what is near a module includes what imports
// it as much as what it imports.
fn neighbourhood<'a>(graph: &FileGraph<'a>, centre: &str) -> BTreeSet<&'a str> {
    let mut adjacent: Adjacent<'a> = BTreeMap::new();
    for (source, target, _) in graph.edges() {
        adjacent.entry(source).or_default().insert(target);
        adjacent.entry(target).or_default().insert(source);
    }
    let start: Vec<&'a str> = (graph.nodes().iter().copied())
        .filter(|node| *node == centre)
        .collect();
    let mut reached: BTreeSet<&'a str> = start.iter().copied().collect();
    let mut frontier = start;
    for _ in 0..FOCUS_DISTANCE {
        frontier = one_hop(&adjacent, &frontier, &mut reached);
    }
    reached
}

type Adjacent<'a> = BTreeMap<&'a str, BTreeSet<&'a str>>;

// The neighbours of `frontier` not reached before, which this hop reaches.
fn one_hop<'a>(
    adjacent: &Adjacent<'a>,
    frontier: &[&'a str],
    reached: &mut BTreeSet<&'a str>,
) -> Vec<&'a str> {
    let neighbours = frontier
        .iter()
        .flat_map(|node| adjacent.get(node).into_iter().flatten());
    neighbours
        .copied()
        .filter(|neighbour| reached.insert(neighbour))
        .collect()
}

// The subgraphs of the communities that hold a drawn module, then the modules in none.
fn chart_nodes(
    selected: &[&str],
    labels: &[String],
    partition: Option<&CommunityPartition<'_>>,
) -> (Vec<ChartGroup>, Vec<ChartNode>) {
    let node = |at: usize| ChartNode {
        id: format!("n{at}"),
        label: labels[at].clone(),
    };
    let Some(partition) = partition else {
        return (Vec::new(), (0..selected.len()).map(node).collect());
    };
    let position = |path: &str| selected.binary_search(&path).ok();
    let drawn: Vec<(&str, Vec<usize>)> = (partition.communities.iter())
        .map(|community| {
            let mut members: Vec<usize> = community
                .members
                .iter()
                .filter_map(|m| position(m))
                .collect();
            members.sort_unstable();
            (community.label.as_str(), members)
        })
        .filter(|(_, members)| !members.is_empty())
        .collect();
    let titles = distinct(drawn.iter().map(|(label, _)| safe_label(label)));
    let grouped: BTreeSet<usize> = drawn
        .iter()
        .flat_map(|(_, members)| members.iter().copied())
        .collect();
    let groups = (drawn.iter().zip(titles).enumerate())
        .map(|(index, ((_, members), title))| ChartGroup {
            id: format!("s{index}"),
            title,
            members: members.iter().map(|&at| node(at)).collect(),
        })
        .collect();
    let rest = (0..selected.len())
        .filter(|at| !grouped.contains(at))
        .map(node)
        .collect();
    (groups, rest)
}

// The edges between two drawn modules in id order. Reference edges are all or nothing against
// the bound, because a sampled subset cannot tell an elision from an absence.
fn chart_edges(
    graph: &FileGraph<'_>,
    ids: &BTreeMap<&str, usize>,
    imports: &BTreeSet<(&str, &str)>,
    max_edges: usize,
) -> (Vec<ChartEdge>, usize) {
    let mut pairs: Vec<(usize, usize, bool)> = (graph.edges())
        .filter(|(source, target, _)| source != target)
        .filter_map(|(source, target, _)| {
            let declared = imports.contains(&(source, target));
            Some((*ids.get(source)?, *ids.get(target)?, declared))
        })
        .collect();
    pairs.sort_unstable();
    let references = pairs.iter().filter(|(_, _, declared)| !declared).count();
    let fits = pairs.len() <= max_edges;
    let edges = (pairs.into_iter())
        .filter(|(_, _, declared)| *declared || fits)
        .map(|(source, target, declared)| ChartEdge {
            from: format!("n{source}"),
            to: format!("n{target}"),
            solid: declared,
        })
        .collect();
    (edges, if fits { 0 } else { references })
}

// Every character outside the allowlist becomes one underscore, runs of underscores and of
// spaces collapse, and a long label is cut with three dots.
pub(crate) fn safe_label(text: &str) -> String {
    let allowed = |c: char| c.is_ascii_alphanumeric() || " ._-/".contains(c);
    let replaced: String = text
        .chars()
        .map(|c| if allowed(c) { c } else { REPLACEMENT })
        .collect();
    let collapsed = collapse(&collapse(&replaced, REPLACEMENT), ' ');
    let trimmed = collapsed.trim_matches(' ');
    if trimmed.is_empty() {
        return UNNAMED_LABEL.to_owned();
    }
    if trimmed.len() > LABEL_MAX_CHARS {
        return format!("{}...", &trimmed[..LABEL_MAX_CHARS - 3]);
    }
    trimmed.to_owned()
}

fn collapse(text: &str, repeated: char) -> String {
    let mut collapsed = String::with_capacity(text.len());
    for c in text.chars() {
        if !(c == repeated && collapsed.ends_with(repeated)) {
            collapsed.push(c);
        }
    }
    collapsed
}

// Every repeat of a label carries an ascending ordinal; the first stays bare.
pub(crate) fn distinct(labels: impl Iterator<Item = String>) -> Vec<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    labels
        .map(|label| {
            let count = counts.entry(label.clone()).or_default();
            *count += 1;
            if *count == 1 {
                label
            } else {
                format!("{label} {count}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
