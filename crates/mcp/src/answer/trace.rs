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
    let at = (arguments.text("rev"), &context.inner.graphs);
    let outcome = app::trace::trace_kept_at(services, &context.reading, &request, at);
    context.logged(&outcome.diagnostics);
    let trace = outcome.value?;
    let answer = TraceAnswer {
        request: &request,
        trace: &trace,
    };
    Ok(context.rendered_at(&answer, trace.revision.as_ref()))
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
