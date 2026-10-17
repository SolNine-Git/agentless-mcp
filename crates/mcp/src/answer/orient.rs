//! The orient operations: the map and the structure views of the whole repository.

use app::ops::limits::{
    DEFAULT_COMMUNITY_LIMIT, DEFAULT_CYCLE_LIMIT, DEFAULT_DIAGRAM_EDGES, DEFAULT_DIAGRAM_NODES,
    DEFAULT_HEALTH_LIMIT, DEFAULT_MAX_VISITED, DEFAULT_MEMBER_LIMIT,
};
use app::{
    CommunitiesRequest, CyclesRequest, DiagramRequest, HealthRequest, IntArg, MapRequest,
    PathPolicy, PathRequest,
};
use io::Granularity;

use super::Context;
use crate::arguments::{Arguments, Value};
use crate::dispatch::Call;
use crate::failure::Failure;

pub(super) fn answer(context: &Context<'_>, call: &Call) -> Result<String, Failure> {
    let arguments = &call.arguments;
    let services = &*context.inner.services;
    let reading = &context.reading;
    match call.operation_name() {
        "map" => map(context, arguments),
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
        other => Err(Failure::Unplanned(format!(
            "orient has no step for {other}"
        ))),
    }
}

// An omitted budget takes the config's, and the limit is the map's file cap.
fn map(context: &Context<'_>, arguments: &Arguments) -> Result<String, Failure> {
    let config = &context.reading.repo.config;
    let budget = arguments.int("budget").cloned().or_else(|| {
        config
            .map_budget
            .map(|budget| IntArg::from_usize(budget as usize))
    });
    let granularity = arguments.text("granularity");
    let request = MapRequest {
        focus: focus_entries(arguments.value("focus")),
        budget,
        max_files: arguments.int("limit").cloned(),
        granularity: match granularity {
            Some("file") => Some(Granularity::File),
            Some("function") => Some(Granularity::Function),
            _ => None,
        },
    };
    let services = &*context.inner.services;
    if granularity == Some("body") {
        return context.viewed(app::body_map(services, &context.reading, &request));
    }
    context.viewed(app::map(services, &context.reading, &request))
}

// A text is one seed, and an empty text no seed.
fn focus_entries(focus: &Value) -> Vec<String> {
    match focus {
        Value::Text(seed) if !seed.is_empty() => vec![seed.clone()],
        Value::List(seeds) => seeds
            .iter()
            .filter_map(|seed| match seed {
                Value::Text(seed) => Some(seed.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
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
