//! The `trace` tool: one card per target, as the trace grammar words it.

use app::trace::{Section, TraceRequest};
use render::TraceAnswer;

use super::Context;
use crate::arguments::{Arguments, Value};
use crate::failure::Failure;

pub(super) fn answer(context: &Context<'_>, arguments: &Arguments) -> Result<String, Failure> {
    let request = TraceRequest {
        targets: targets(arguments.value("target")),
        more: arguments.text("more").and_then(Section::from_name),
    };
    let services = &context.inner.services;
    let graphs = &context.inner.graphs;
    let outcome = app::trace::trace_kept(services, &context.reading, &request, graphs);
    context.logged(&outcome.diagnostics);
    let trace = outcome.value?;
    Ok(context.rendered(&TraceAnswer {
        request: &request,
        trace: &trace,
    }))
}

// One target as given, or each target of a list; the validation lets only these two through.
fn targets(target: &Value) -> Vec<String> {
    match target {
        Value::Text(target) => vec![target.clone()],
        Value::List(targets) => (targets.iter())
            .filter_map(|target| match target {
                Value::Text(target) => Some(target.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}
