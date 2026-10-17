//! The file graph that communities, diagrams and the HTML export draw: one scan's graph under the
//! repository's weighting, its ranking, the community resolution, and the modules a bound keeps.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use graph::{FileFacts, FileGraph, NameIndex, PageRank, Scopes, Weighting, pagerank};
use io::ProjectConfig;

use crate::bounds::within_float;
use crate::ops::limits::{DEFAULT_RESOLUTION, MAX_RESOLUTION};
use crate::{Error, is_test_path};

// Two hops is what "what touches this" means: what it calls, and what those call.
const FOCUS_DISTANCE: usize = 2;

// The file graph of one scan under the repository's weighting.
pub(crate) fn file_graph<'a>(
    index: &NameIndex<'a>,
    scopes: &Scopes<'a>,
    config: &ProjectConfig,
) -> FileGraph<'a> {
    let weighting = Weighting {
        relation_weights: config.relation_weights.unwrap_or(false),
        stoplist: &config.stoplist,
    };
    FileGraph::build(index, scopes, &weighting)
}

// NaN makes every gain comparison false and a negative resolution reports a modularity above 1,
// so both are refused. The floor is exclusive, unlike the range check after it.
pub(crate) fn resolution_setting(resolution: Option<f64>) -> Result<f64, Error> {
    let Some(setting) = resolution else {
        return Ok(DEFAULT_RESOLUTION);
    };
    if !setting.is_finite() || setting <= 0.0 {
        let message = format!(
            "resolution must be a finite number greater than 0, got {}",
            json::float_text(setting)
        );
        return Err(kernel::Error::OperationFailed(message).into());
    }
    within_float(setting, 0.0, MAX_RESOLUTION, "resolution")
}

// One call's file graph, its ranking and the file pairs a declared import connects.
pub(crate) struct Ranked<'a> {
    pub(crate) index: NameIndex<'a>,
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

// Ranks are finite. -0.0 and 0.0 compare equal.
fn descending(left: f64, right: f64) -> Ordering {
    right.partial_cmp(&left).unwrap_or(Ordering::Equal)
}

type Adjacent<'a> = BTreeMap<&'a str, BTreeSet<&'a str>>;

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
