//! The fewest-hop chain of resolved relationships between two symbols or files.

use graph::{Hop, PathResult, Relation, shortest_path};

pub use graph::PathPolicy;

use crate::bounds::at_least;
use crate::communities::over_scan;
use crate::endpoints::{Located, endpoint, with_resolved};
use crate::{Error, IntArg, Noted, Reading, Services};

/// A path request: the two endpoints, the weak tiers the search may walk, and its node bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathRequest {
    /// The start: a stable id, a path, or a name.
    pub source: String,
    /// The end: a stable id, a path, or a name.
    pub target: String,
    /// Whether the search walks unique-name and name-only-ambiguous edges.
    pub policy: PathPolicy,
    /// The most nodes the search visits.
    pub max_visited: IntArg,
}

/// The hops between two endpoints, or the reason there are none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathTrace {
    /// The start's node, or the text the caller gave when an endpoint did not resolve.
    pub source: String,
    /// The end's node, or the text the caller gave when an endpoint did not resolve.
    pub target: String,
    /// The start's label.
    pub source_label: String,
    /// The end's label.
    pub target_label: String,
    /// Each step, start to end.
    pub hops: Vec<PathHop>,
    /// Whether the search reached the end.
    pub found: bool,
    /// Why there are no hops, or empty.
    pub message: String,
    /// How many nodes the search visited.
    pub visited: usize,
    /// Whether the search stopped at its node bound.
    pub exhausted: bool,
    /// The weak tiers the search walked.
    pub policy: PathPolicy,
    /// Whether both endpoints resolved.
    pub endpoints_resolved: bool,
}

/// One step of a path, with the direction its edge really runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathHop {
    /// The relation as the step reads it, such as `referenced by`.
    pub verb: &'static str,
    /// The tier's wire value.
    pub tier: &'static str,
    /// The tier's label.
    pub tier_label: &'static str,
    /// `->` with the edge, `<-` against it.
    pub arrow: &'static str,
    /// The node the step reaches.
    pub node: String,
    /// The label of that node.
    pub label: String,
    /// The path of that node.
    pub path: String,
    /// The line of that node.
    pub line: u32,
}

/// Finds the fewest-hop path between the two endpoints of `request`. The scan's warnings come
/// with either outcome.
#[must_use]
pub fn path(
    services: &Services,
    reading: &Reading<'_>,
    request: &PathRequest,
) -> Noted<Result<PathTrace, Error>> {
    let max_visited = match at_least(&request.max_visited, 1, "max_visited") {
        Ok(bound) => bound,
        Err(error) => return Noted::new(Err(error)),
    };
    over_scan(services, reading, |facts| {
        Ok(with_resolved(&facts.files, |index, graph| {
            let start = endpoint(index, graph, &request.source);
            let finish = endpoint(index, graph, &request.target);
            if !start.message.is_empty() || !finish.message.is_empty() {
                return unresolved(request, start, finish);
            }
            let policy = request.policy;
            let found = shortest_path(graph, &start.node, &finish.node, policy, max_visited);
            trace(policy, start, finish, &found)
        }))
    })
}

// The relation as a step with the edge reads it, and as a step against the edge reads it.
pub(crate) fn verbs(relation: Relation) -> (&'static str, &'static str) {
    match relation {
        Relation::References => ("references", "referenced by"),
        Relation::Imports => ("imports", "imported by"),
        Relation::Inherits => ("inherits", "inherited by"),
    }
}

fn trace(policy: PathPolicy, start: Located, finish: Located, found: &PathResult<'_>) -> PathTrace {
    let message = path_message(policy, &start.node, &finish.node, found);
    PathTrace {
        source: start.node,
        target: finish.node,
        source_label: start.label,
        target_label: finish.label,
        hops: found.hops.iter().map(hop).collect(),
        found: found.found,
        message,
        visited: found.visited,
        exhausted: found.exhausted,
        policy,
        endpoints_resolved: true,
    }
}

fn hop(step: &Hop<'_>) -> PathHop {
    let edge = &step.edge;
    let arrival = step.arrival();
    let (active, passive) = verbs(edge.relation());
    let (verb, arrow) = if step.forward {
        (active, "->")
    } else {
        (passive, "<-")
    };
    PathHop {
        verb,
        tier: edge.tier().as_str(),
        tier_label: edge.tier().label(),
        arrow,
        node: arrival.node().to_owned(),
        label: arrival.label().to_owned(),
        path: arrival.path().to_owned(),
        line: arrival.line(),
    }
}

// "Nothing connects these" and "the search stopped" are different facts, and the second names
// the bound it hit and where a caller can raise it.
fn path_message(policy: PathPolicy, source: &str, target: &str, found: &PathResult<'_>) -> String {
    if found.found {
        return String::new();
    }
    if found.exhausted {
        return format!(
            "no path from {source} to {target} within the search bound ({} nodes visited); pick \
             a nearer pair of endpoints, or raise --max-visited from the CLI",
            found.visited
        );
    }
    let unique = (!policy.include_unique).then_some("unique");
    let ambiguous = (!policy.include_ambiguous).then_some("name-only-ambiguous");
    let excluded: Vec<&str> = unique.into_iter().chain(ambiguous).collect();
    let named = (!excluded.is_empty()).then(|| excluded.join(" and "));
    let hint = named.map(|tiers| format!("; {tiers} edges were excluded"));
    format!(
        "no path from {source} to {target} over resolved edges{}",
        hint.unwrap_or_default()
    )
}

fn unresolved(request: &PathRequest, start: Located, finish: Located) -> PathTrace {
    let reasons: Vec<&str> = [start.message.as_str(), finish.message.as_str()]
        .into_iter()
        .filter(|message| !message.is_empty())
        .collect();
    PathTrace {
        source: request.source.clone(),
        target: request.target.clone(),
        source_label: start.label,
        target_label: finish.label,
        hops: Vec::new(),
        found: false,
        message: reasons.join("; "),
        visited: 0,
        exhausted: false,
        policy: request.policy,
        endpoints_resolved: false,
    }
}

#[cfg(test)]
mod tests;
