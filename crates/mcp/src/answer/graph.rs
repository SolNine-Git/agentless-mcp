//! The graph operations: the structure views of the whole repository.

use app::ops::limits::{
    DEFAULT_COMMUNITY_LIMIT, DEFAULT_CYCLE_LIMIT, DEFAULT_DIAGRAM_EDGES, DEFAULT_DIAGRAM_NODES,
    DEFAULT_HEALTH_LIMIT, DEFAULT_MAX_VISITED, DEFAULT_MEMBER_LIMIT,
};
use app::{
    CommunitiesRequest, CyclesRequest, DiagramRequest, HealthRequest, IntArg, PathPolicy,
    PathRequest,
};

use super::Context;
use super::orient::focus_entries;
use crate::arguments::Arguments;
use crate::dispatch::Call;
use crate::failure::Failure;

pub(super) fn answer(context: &Context<'_>, call: &Call) -> Result<String, Failure> {
    let arguments = &call.arguments;
    let services = &*context.inner.services;
    let reading = &context.reading;
    match call.operation_name() {
        "communities" => context.viewed(app::communities(
            services,
            reading,
            &CommunitiesRequest {
                resolution: arguments.float("resolution"),
                limit: arguments.int_or("limit", DEFAULT_COMMUNITY_LIMIT),
                members: IntArg::from_usize(DEFAULT_MEMBER_LIMIT),
            },
        )),
        "cycles" => {
            let limit = arguments.int_or("limit", DEFAULT_CYCLE_LIMIT);
            context.viewed(app::cycles(services, reading, &CyclesRequest { limit }))
        }
        "health" => {
            let limit = arguments.int_or("limit", DEFAULT_HEALTH_LIMIT);
            context.viewed(app::health(services, reading, &HealthRequest { limit }))
        }
        "diagram" => context.viewed(app::diagram(services, reading, &diagram(arguments))),
        "path" => context.viewed(app::path(services, reading, &path(arguments))),
        other => Err(Failure::Unplanned(format!("graph has no step for {other}"))),
    }
}

// A diagram has one centre: the text, or the first entry of a list; an empty one is none.
fn diagram(arguments: &Arguments) -> DiagramRequest {
    let focus = focus_entries(arguments.value("focus")).into_iter().next();
    DiagramRequest {
        focus: focus.filter(|seed| !seed.is_empty()),
        max_nodes: arguments.int_or("max_nodes", DEFAULT_DIAGRAM_NODES),
        max_edges: arguments.int_or("max_edges", DEFAULT_DIAGRAM_EDGES),
        group_by_communities: arguments.flag("group_by_communities"),
        resolution: arguments.float("resolution"),
    }
}

fn path(arguments: &Arguments) -> PathRequest {
    PathRequest {
        source: arguments.text_or_empty("source"),
        target: arguments.text_or_empty("target"),
        policy: PathPolicy {
            include_unique: arguments.flag("include_unique"),
            include_ambiguous: arguments.flag("include_ambiguous"),
        },
        max_visited: IntArg::from_usize(DEFAULT_MAX_VISITED),
    }
}
