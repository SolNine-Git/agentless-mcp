//! One symbol's card: where it is defined, what it touches and what touches it, each side
//! grouped by evidence tier, and the imports of its file.

use graph::{Definition, NameIndex, SymbolEdge, SymbolGraph, Tier, definitions_for};
use kernel::Bounded;

use crate::bounds::within;
use crate::endpoints::{best_band, rank_candidates};
use crate::ops::limits::MAX_LIMIT;
use crate::scan::over_scan;
use crate::symbol_graph::{tier_at, verbs, with_resolved};
use crate::{
    Error, IntArg, Noted, RationaleNode, Reading, Services, SymbolCard, rationale_nodes,
    symbol_card,
};

/// An explain request: the symbol, and how many rows each tier and each import side lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExplainRequest {
    /// A stable id, a qualified name, or a name.
    pub target: String,
    /// The most rows per tier and per import side.
    pub limit: IntArg,
}

/// One symbol's card with its tiered fan-out and fan-in and its file's imports, or the message
/// that nothing matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explanation {
    /// The text the caller gave.
    pub target: String,
    /// The card of the symbol explained, or `None` when nothing matched.
    pub card: Option<SymbolCard>,
    /// Why there is no card, or empty.
    pub message: String,
    /// The stable ids of the other definitions that matched, best first.
    pub alternatives: Vec<String>,
    /// The rationale comments of the symbol.
    pub rationales: Vec<RationaleNode>,
    /// The edges leaving the symbol, strongest tier first.
    pub fan_out: Vec<TierGroup>,
    /// The edges arriving at the symbol, strongest tier first.
    pub fan_in: Vec<TierGroup>,
    /// The imports the symbol's file declares.
    pub imports_out: Bounded<ImportRow>,
    /// The imports of the symbol's file by other files.
    pub imports_in: Bounded<ImportRow>,
    /// How many definitions matched as well as the one explained.
    pub tied: usize,
}

/// The edges of one evidence tier, cut at the limit, with the count before the cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TierGroup {
    /// The tier's wire value.
    pub tier: &'static str,
    /// The tier's label.
    pub tier_label: &'static str,
    /// The rows the limit kept.
    pub rows: Vec<EdgeRow>,
    /// The edges of this tier.
    pub total: usize,
}

/// One edge, seen from the symbol the card is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdgeRow {
    /// The other end's node.
    pub node: String,
    /// The other end's label.
    pub label: String,
    /// The other end's path.
    pub path: String,
    /// The other end's line.
    pub line: u32,
    /// The relation as the card reads it, such as `referenced by`.
    pub relation: &'static str,
}

/// One module-level import edge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportRow {
    /// The importing file.
    pub path: String,
    /// The line of the import statement.
    pub line: u32,
    /// The module text of the statement.
    pub module: String,
    /// The imported file.
    pub other: String,
}

/// Explains the symbol that `request` names in the repository that `reading` names. The scan's
/// warnings come with either outcome.
#[must_use]
pub fn explain(
    services: &Services,
    reading: &Reading<'_>,
    request: &ExplainRequest,
) -> Noted<Result<Explanation, Error>> {
    let limit = match within(&request.limit, 1, MAX_LIMIT, "limit") {
        Ok(limit) => limit,
        Err(error) => return Noted::new(Err(error)),
    };
    let target = request.target.as_str();
    over_scan(services, reading, |facts| {
        with_resolved(&facts.files, reading.deadline, |index, graph| {
            Ok(explanation(index, graph, target, limit))
        })
    })
}

fn explanation(
    index: &NameIndex<'_>,
    graph: &SymbolGraph<'_>,
    target: &str,
    limit: usize,
) -> Explanation {
    let definitions = definitions_for(index, target);
    if definitions.is_empty() {
        return missing(target);
    }
    let ordered = rank_candidates(definitions, target);
    let tied = best_band(&ordered, target).len();
    let chosen = &ordered[0];
    let node = chosen.stable_id().to_string();
    let degrees = graph.ambiguous_degrees();
    let (imports_out, imports_in) = imports(graph, chosen.path(), limit);
    let mut fan_out = tier_groups(graph.outgoing(&node).iter(), limit, true);
    let leaving = graph.ambiguous_from(&node);
    fan_out.extend(ambiguous_group(
        leaving,
        degrees.leaving(&node),
        limit,
        true,
    ));
    let mut fan_in = tier_groups(graph.incoming(&node), limit, false);
    let arriving = graph.ambiguous_into(&node);
    fan_in.extend(ambiguous_group(
        arriving,
        degrees.arriving(&node),
        limit,
        false,
    ));
    Explanation {
        target: target.to_owned(),
        card: Some(card(chosen)),
        message: String::new(),
        alternatives: ordered[1..]
            .iter()
            .map(|entry| entry.stable_id().to_string())
            .collect(),
        rationales: rationale_nodes(chosen.file().language, chosen.path(), chosen.symbol()),
        fan_out,
        fan_in,
        imports_out,
        imports_in,
        tied,
    }
}

fn card(definition: &Definition<'_>) -> SymbolCard {
    let language = definition.file().language;
    symbol_card(
        language,
        definition.path(),
        definition.symbol(),
        String::new(),
    )
}

fn missing(target: &str) -> Explanation {
    Explanation {
        target: target.to_owned(),
        card: None,
        message: format!("no symbol matches {target}"),
        alternatives: Vec::new(),
        rationales: Vec::new(),
        fan_out: Vec::new(),
        fan_in: Vec::new(),
        imports_out: Bounded::cut(Vec::new(), 0),
        imports_in: Bounded::cut(Vec::new(), 0),
        tied: 1,
    }
}

// The edges of one side by tier, in the strongest-first tier order, each kept in edge order.
fn tier_groups<'e, 'a: 'e>(
    edges: impl Iterator<Item = &'e SymbolEdge<'a>>,
    limit: usize,
    outgoing: bool,
) -> Vec<TierGroup> {
    let mut buckets: Vec<(Tier, Vec<EdgeRow>)> = Tier::ORDER.map(|tier| (tier, Vec::new())).into();
    for edge in edges {
        buckets[tier_at(edge.tier())]
            .1
            .push(edge_row(edge, outgoing));
    }
    (buckets.into_iter())
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(tier, rows)| group(tier, rows, limit))
        .collect()
}

// The ambiguous edges are generated on demand, so the count comes from the degree tally and only
// the first `limit` in sort-key order become rows.
fn ambiguous_group<'a>(
    edges: impl Iterator<Item = SymbolEdge<'a>>,
    total: u64,
    limit: usize,
    outgoing: bool,
) -> Option<TierGroup> {
    if total == 0 {
        return None;
    }
    let mut first: Vec<SymbolEdge<'a>> = edges.collect();
    first.sort_by(SymbolEdge::cmp_sort_key);
    first.truncate(limit);
    let rows = first.iter().map(|edge| edge_row(edge, outgoing)).collect();
    Some(TierGroup {
        tier: Tier::Ambiguous.as_str(),
        tier_label: Tier::Ambiguous.label(),
        rows,
        // usize holds every u64 on a 64-bit target; on a narrower one the count saturates.
        total: usize::try_from(total).unwrap_or(usize::MAX),
    })
}

fn group(tier: Tier, mut rows: Vec<EdgeRow>, limit: usize) -> TierGroup {
    let total = rows.len();
    rows.truncate(limit);
    TierGroup {
        tier: tier.as_str(),
        tier_label: tier.label(),
        rows,
        total,
    }
}

fn edge_row(edge: &SymbolEdge<'_>, outgoing: bool) -> EdgeRow {
    let (active, passive) = verbs(edge.relation());
    let (other, relation) = if outgoing {
        (edge.target(), active)
    } else {
        (edge.source(), passive)
    };
    EdgeRow {
        node: other.node().to_owned(),
        label: other.label().to_owned(),
        path: other.path().to_owned(),
        line: other.line(),
        relation,
    }
}

// The import edges that leave `path` (declared) and those that arrive at it, each counted
// before the cut.
fn imports(
    graph: &SymbolGraph<'_>,
    path: &str,
    limit: usize,
) -> (Bounded<ImportRow>, Bounded<ImportRow>) {
    let (mut declared, mut importers) = (Vec::new(), Vec::new());
    for edge in graph.import_edges() {
        let row = || ImportRow {
            path: edge.source().path().to_owned(),
            line: edge.source().line(),
            module: edge.name().to_owned(),
            other: edge.target().path().to_owned(),
        };
        if edge.source().node() == path {
            declared.push(row());
        }
        if edge.target().node() == path {
            importers.push(row());
        }
    }
    (
        Bounded::cut(declared, limit),
        Bounded::cut(importers, limit),
    )
}

#[cfg(test)]
mod tests;
