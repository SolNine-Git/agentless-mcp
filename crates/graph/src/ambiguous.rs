//! The name-only-ambiguous edges: each reference stored once, expanded against every candidate
//! definition of its name on demand.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::resolver::Tier;
use crate::symbols::{AmbiguousReference, Endpoint, SymbolEdge, SymbolGraph};

/// How many ambiguous edges arrive at and leave each node; a node with none is absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AmbiguousDegrees {
    pub arriving: BTreeMap<String, u64>,
    pub leaving: BTreeMap<String, u64>,
}

impl AmbiguousDegrees {
    /// Return how many ambiguous edges arrive at `node`.
    #[must_use]
    pub fn arriving(&self, node: &str) -> u64 {
        self.arriving.get(node).copied().unwrap_or(0)
    }

    /// Return how many ambiguous edges leave `node`.
    #[must_use]
    pub fn leaving(&self, node: &str) -> u64 {
        self.leaving.get(node).copied().unwrap_or(0)
    }
}

impl<'a> SymbolGraph<'a> {
    pub(crate) fn index_ambiguous(&mut self) {
        for (at, reference) in self.ambiguous.iter().enumerate() {
            self.by_name.entry(reference.name()).or_default().push(at);
        }
        for (name, targets) in &self.candidates {
            for target in targets {
                let held = self.candidates_at.entry(Arc::clone(target.node_arc()));
                held.or_default().push((*name, target.clone()));
            }
        }
    }

    /// Return the ambiguous references `node` makes, in sort-key order.
    #[must_use]
    pub fn references_from(&self, node: &str) -> &[AmbiguousReference<'a>] {
        let start = self
            .ambiguous
            .partition_point(|item| item.source().node() < node);
        let end = self
            .ambiguous
            .partition_point(|item| item.source().node() <= node);
        &self.ambiguous[start..end]
    }

    /// Return each candidate endpoint at `node`, beside the name it answers, in the order the
    /// names were first met.
    #[must_use]
    pub fn candidates_at(&self, node: &str) -> &[(&'a str, Endpoint<'a>)] {
        self.candidates_at.get(node).map_or(&[], Vec::as_slice)
    }

    /// Return the edges one ambiguous reference stands for, in candidate order.
    pub fn ambiguous_edges<'g>(
        &'g self,
        reference: &'g AmbiguousReference<'a>,
    ) -> impl Iterator<Item = SymbolEdge<'a>> + 'g {
        self.candidates_of(reference.name())
            .iter()
            .filter_map(move |target| ambiguous_edge(reference, target))
    }

    /// Return the ambiguous edges leaving `node`.
    pub fn ambiguous_from<'g>(
        &'g self,
        node: &'g str,
    ) -> impl Iterator<Item = SymbolEdge<'a>> + 'g {
        self.references_from(node)
            .iter()
            .flat_map(|reference| self.ambiguous_edges(reference))
    }

    /// Return the ambiguous edges arriving at `node`.
    pub fn ambiguous_into<'g>(
        &'g self,
        node: &'g str,
    ) -> impl Iterator<Item = SymbolEdge<'a>> + 'g {
        self.candidates_at(node)
            .iter()
            .flat_map(move |(name, target)| {
                self.references_named(name)
                    .filter_map(move |reference| ambiguous_edge(reference, target))
            })
    }

    /// Count each node's ambiguous edges, arriving and leaving, without building one.
    #[must_use]
    pub fn ambiguous_degrees(&self) -> AmbiguousDegrees {
        let mut degrees = AmbiguousDegrees::default();
        for name in self.by_name.keys() {
            let targets = self.candidates_of(name);
            let at_node = tally(targets.iter().map(Endpoint::node));
            let references: Vec<&AmbiguousReference<'a>> = self.references_named(name).collect();
            let sources = tally(references.iter().map(|reference| reference.source().node()));
            let width = targets.len() as u64;
            for reference in &references {
                let node = reference.source().node();
                add(
                    &mut degrees.leaving,
                    node,
                    width - at_node.get(node).copied().unwrap_or(0),
                );
            }
            let spelled = references.len() as u64;
            for (node, count) in &at_node {
                let own = sources.get(node).copied().unwrap_or(0);
                add(&mut degrees.arriving, node, count * (spelled - own));
            }
        }
        degrees
    }

    fn candidates_of(&self, name: &str) -> &[Endpoint<'a>] {
        self.candidate_names
            .get(name)
            .map_or(&[], |&at| self.candidates[at].1.as_slice())
    }

    fn references_named(&self, name: &str) -> impl Iterator<Item = &AmbiguousReference<'a>> + '_ {
        let positions = self.by_name.get(name).map_or(&[][..], Vec::as_slice);
        positions.iter().map(|&at| &self.ambiguous[at])
    }
}

fn ambiguous_edge<'a>(
    reference: &AmbiguousReference<'a>,
    target: &Endpoint<'a>,
) -> Option<SymbolEdge<'a>> {
    SymbolEdge::new(
        reference.source().clone(),
        target.clone(),
        Cow::Borrowed(reference.name()),
        reference.relation(),
        Tier::Ambiguous,
    )
}

fn tally<'n>(nodes: impl Iterator<Item = &'n str>) -> BTreeMap<&'n str, u64> {
    let mut counts = BTreeMap::new();
    for node in nodes {
        *counts.entry(node).or_insert(0) += 1;
    }
    counts
}

// A zero count is no entry, as a Counter's unary plus leaves it.
fn add(counts: &mut BTreeMap<String, u64>, node: &str, count: u64) {
    if count > 0 {
        *counts.entry(node.to_owned()).or_insert(0) += count;
    }
}
