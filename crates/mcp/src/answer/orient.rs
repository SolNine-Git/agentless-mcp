//! The orient operation: the map of the files and symbols most relevant to a focus.

use app::{IntArg, MapRequest};
use io::Granularity;

use super::Context;
use crate::arguments::{Arguments, Value};
use crate::dispatch::Call;
use crate::failure::Failure;

pub(super) fn answer(context: &Context<'_>, call: &Call) -> Result<String, Failure> {
    match call.operation_name() {
        "map" => map(context, &call.arguments),
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
pub(super) fn focus_entries(focus: &Value) -> Vec<String> {
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
