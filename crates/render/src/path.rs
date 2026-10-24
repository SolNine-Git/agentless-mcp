//! The path's two forms: one row per hop with its relation and tier in the text, and every hop
//! in the JSON, trimmed at the hops.

use app::{PathHop, PathTrace, locator};
use serde::Serialize;
use text::one_line;

use crate::envelope::{Listing, wrap_json};
use crate::{Rendered, View};

impl View for PathTrace {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(path_text(self), PathJson::of(self), None, wrap_json)
    }
}

fn path_text(trace: &PathTrace) -> String {
    if !trace.found {
        return one_line(&trace.message).trim_end_matches('\n').to_owned() + "\n";
    }
    let count = trace.hops.len();
    let noun = if count == 1 { "hop" } else { "hops" };
    let (source, target) = (one_line(&trace.source), one_line(&trace.target));
    let mut lines = vec![
        format!("{count} {noun} from {source} to {target}"),
        format!("  start  {}    {source}", one_line(&trace.source_label)),
    ];
    let numbered = trace.hops.iter().enumerate();
    lines.extend(numbered.map(|(at, hop)| hop_line(at + 1, hop)));
    if !trace.message.is_empty() {
        lines.push(format!("  {}", one_line(&trace.message)));
    }
    lines.join("\n") + "\n"
}

fn hop_line(number: usize, hop: &PathHop) -> String {
    let line = hop.line.to_string();
    format!(
        "  {number:>3}. {} {} ({})    {}    {}",
        one_line(hop.arrow),
        one_line(hop.verb),
        one_line(hop.tier_label),
        one_line(&hop.label),
        locator(&hop.node, "", Some(&line))
    )
}

#[derive(Clone, Serialize)]
struct PathJson<'a> {
    source: &'a str,
    target: &'a str,
    endpoints_resolved: bool,
    found: bool,
    message: &'a str,
    hops: Vec<HopJson<'a>>,
    visited: usize,
    exhausted: bool,
    #[serde(flatten)]
    policy: PolicyJson,
}

// The two weak tiers the search could walk, as two keys of the trace.
#[derive(Clone, Copy, Serialize)]
struct PolicyJson {
    include_unique: bool,
    include_ambiguous: bool,
}

#[derive(Clone, Serialize)]
struct HopJson<'a> {
    relation: &'a str,
    tier: &'a str,
    direction: &'a str,
    node: &'a str,
    label: &'a str,
    path: &'a str,
    line: u32,
}

impl<'a> PathJson<'a> {
    fn of(trace: &'a PathTrace) -> Self {
        let hops = trace.hops.iter().map(|hop| HopJson {
            relation: hop.verb,
            tier: hop.tier,
            direction: hop.arrow,
            node: &hop.node,
            label: &hop.label,
            path: &hop.path,
            line: hop.line,
        });
        PathJson {
            source: &trace.source,
            target: &trace.target,
            endpoints_resolved: trace.endpoints_resolved,
            found: trace.found,
            message: &trace.message,
            hops: hops.collect(),
            visited: trace.visited,
            exhausted: trace.exhausted,
            policy: PolicyJson {
                include_unique: trace.policy.include_unique,
                include_ambiguous: trace.policy.include_ambiguous,
            },
        }
    }
}

impl Listing for PathJson<'_> {
    fn items(&self) -> usize {
        self.hops.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.hops.truncate(kept);
        prefix
    }
}
