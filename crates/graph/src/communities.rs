//! Communities of the file graph: modularity local moving over the symmetrised graph, each
//! community labelled by the directory a strict majority of its members share.

use std::collections::BTreeMap;

use metrics::compensated_sum;

use crate::files::FileGraph;

/// The resolution of the modularity objective when the caller names none.
pub const DEFAULT_RESOLUTION: f64 = 1.0;
/// The label of a community whose members share no majority directory.
pub const ROOT_LABEL: &str = "repository root";

// The bound exists so that a pathological graph cannot spin, not as a tuning knob.
const MAX_PASSES: u32 = 50;
// Gains are differences of float sums, so "strictly better" needs a floor above their noise.
const GAIN_FLOOR: f64 = 1e-12;

/// One community: its label, its members in path order, and its weights on the symmetrised
/// graph (edges inside it, and its members' summed degree).
#[derive(Clone, Debug, PartialEq)]
pub struct Community<'a> {
    pub label: String,
    pub members: Vec<&'a str>,
    pub internal_weight: f64,
    pub total_weight: f64,
}

impl Community<'_> {
    /// Return how many files the community holds.
    #[must_use]
    pub fn size(&self) -> usize {
        self.members.len()
    }
}

/// A whole partition, largest community first, with its scaled and standard modularity.
#[derive(Clone, Debug, PartialEq)]
pub struct CommunityPartition<'a> {
    pub communities: Vec<Community<'a>>,
    pub modularity: f64,
    pub standard_modularity: f64,
    pub resolution: f64,
    pub converged: bool,
}

impl<'a> CommunityPartition<'a> {
    /// Map each member path to the position of its community.
    #[must_use]
    pub fn index_of(&self) -> BTreeMap<&'a str, usize> {
        self.communities
            .iter()
            .enumerate()
            .flat_map(|(position, community)| {
                community
                    .members
                    .iter()
                    .map(move |member| (*member, position))
            })
            .collect()
    }
}

// The undirected view: per node, its neighbours in first-met order with summed weights.
struct Weighted {
    adjacency: Vec<Vec<(usize, f64)>>,
    degree: Vec<f64>,
    two_m: f64,
}

/// Partition `graph`'s files into communities at `resolution`, largest first, ties by members.
#[must_use]
pub fn detect_communities<'a>(graph: &FileGraph<'a>, resolution: f64) -> CommunityPartition<'a> {
    let nodes = graph.nodes();
    let weighted = symmetrise(graph);
    let mut membership: Vec<usize> = (0..nodes.len()).collect();
    if weighted.two_m <= 0.0 {
        return partition(nodes, &membership, &weighted, resolution, true);
    }
    let mut total = weighted.degree.clone();
    let mut converged = false;
    for _ in 0..MAX_PASSES {
        if one_pass(&weighted, &mut total, &mut membership, resolution) == 0 {
            converged = true;
            break;
        }
    }
    partition(nodes, &membership, &weighted, resolution, converged)
}

fn symmetrise(graph: &FileGraph<'_>) -> Weighted {
    let mut cells: Vec<Cells> = (0..graph.nodes().len()).map(|_| Cells::default()).collect();
    for (source, target, weight) in graph.edges() {
        let from = graph.position(source).expect("an edge starts at a node");
        let to = graph.position(target).expect("an edge ends at a node");
        cells[from].add(to, weight);
        cells[to].add(from, weight);
    }
    let adjacency: Vec<Vec<(usize, f64)>> = cells.into_iter().map(|held| held.order).collect();
    let degree: Vec<f64> = adjacency
        .iter()
        .map(|order| compensated_sum(&order.iter().map(|&(_, weight)| weight).collect::<Vec<_>>()))
        .collect();
    let two_m = compensated_sum(&degree);
    Weighted {
        adjacency,
        degree,
        two_m,
    }
}

// One node's neighbours in the order the edges first met them, with a lookup by neighbour.
#[derive(Default)]
struct Cells {
    order: Vec<(usize, f64)>,
    at: BTreeMap<usize, usize>,
}

impl Cells {
    fn add(&mut self, other: usize, weight: f64) {
        if let Some(&held) = self.at.get(&other) {
            self.order[held].1 += weight;
        } else {
            self.at.insert(other, self.order.len());
            self.order.push((other, 0.0 + weight));
        }
    }
}

// Move each node to its best community once; return how many moved. The node leaves its own
// community before any candidate is scored, so that community competes on the same terms.
fn one_pass(
    weighted: &Weighted,
    total: &mut [f64],
    membership: &mut [usize],
    resolution: f64,
) -> usize {
    let mut moved = 0;
    for node in 0..membership.len() {
        let origin = membership[node];
        let node_degree = weighted.degree[node];
        total[origin] -= node_degree;
        let into = neighbour_weights(weighted, membership, node, origin);
        let score = |(community, weight): (usize, f64)| {
            weight - resolution * node_degree * total[community] / weighted.two_m
        };
        let mut best = (origin, score((origin, into[&origin])));
        for (&community, &weight) in &into {
            let candidate = score((community, weight));
            if candidate > best.1 + GAIN_FLOOR {
                best = (community, candidate);
            }
        }
        total[best.0] += node_degree;
        membership[node] = best.0;
        moved += usize::from(best.0 != origin);
    }
    moved
}

// The weight from `node` into each neighbouring community, its own community always present.
fn neighbour_weights(
    weighted: &Weighted,
    membership: &[usize],
    node: usize,
    origin: usize,
) -> BTreeMap<usize, f64> {
    let mut into = BTreeMap::from([(origin, 0.0)]);
    for &(neighbour, weight) in &weighted.adjacency[node] {
        *into.entry(membership[neighbour]).or_insert(0.0) += weight;
    }
    into
}

fn partition<'a>(
    nodes: &[&'a str],
    membership: &[usize],
    weighted: &Weighted,
    resolution: f64,
    converged: bool,
) -> CommunityPartition<'a> {
    let mut grouped: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (node, &community) in membership.iter().enumerate() {
        grouped.entry(community).or_default().push(node);
    }
    let mut communities = Vec::new();
    let (mut modularity, mut standard) = (0.0, 0.0);
    for (&community, members) in &grouped {
        let internal = compensated_sum(&internal_weights(weighted, membership, members, community));
        let outgoing = compensated_sum(
            &members
                .iter()
                .map(|&member| weighted.degree[member])
                .collect::<Vec<_>>(),
        );
        if weighted.two_m > 0.0 {
            // Both scores share one loop, so at resolution 1.0 they are the same float.
            let fraction = internal / weighted.two_m;
            let share = (outgoing / weighted.two_m).powf(std::hint::black_box(2.0));
            modularity += fraction - resolution * share;
            standard += fraction - share;
        }
        let paths: Vec<&'a str> = members.iter().map(|&member| nodes[member]).collect();
        communities.push(Community {
            label: community_label(&paths),
            members: paths,
            internal_weight: internal,
            total_weight: outgoing,
        });
    }
    communities
        .sort_by(|left, right| (right.size(), &left.members).cmp(&(left.size(), &right.members)));
    CommunityPartition {
        communities,
        modularity,
        standard_modularity: standard,
        resolution,
        converged,
    }
}

fn internal_weights(
    weighted: &Weighted,
    membership: &[usize],
    members: &[usize],
    community: usize,
) -> Vec<f64> {
    members
        .iter()
        .flat_map(|&member| &weighted.adjacency[member])
        .filter(|(neighbour, _)| membership[*neighbour] == community)
        .map(|&(_, weight)| weight)
        .collect()
}

/// Return the deepest directory prefix that a strict majority of `members` share, or
/// [`ROOT_LABEL`] when none does.
#[must_use]
pub fn community_label(members: &[&str]) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for member in members {
        let parts: Vec<&str> = member.split('/').collect();
        for depth in 1..parts.len() {
            *counts.entry(parts[..depth].join("/")).or_insert(0) += 1;
        }
    }
    let threshold = members.len() / 2 + 1;
    counts
        .into_iter()
        .filter(|(_, count)| *count >= threshold)
        .map(|(prefix, _)| prefix)
        .min_by(|left, right| {
            let depth = |prefix: &str| prefix.matches('/').count();
            depth(right).cmp(&depth(left)).then_with(|| left.cmp(right))
        })
        .unwrap_or_else(|| ROOT_LABEL.to_owned())
}
