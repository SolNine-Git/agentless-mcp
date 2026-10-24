//! The symbols operations: find, overview, expand, explain and locate.

use app::ops::limits::{
    DEFAULT_CONTEXT_LINES, DEFAULT_EXPAND_LIMIT, DEFAULT_EXPLAIN_LIMIT, DEFAULT_FIND_LIMIT,
};
use app::{ExpandRequest, ExplainRequest, FindRequest, LocsRequest, SkeletonRequest};
use facts::SymbolKind;
use render::{ToolForm, tool_text};

use super::Context;
use crate::arguments::Arguments;
use crate::dispatch::Call;
use crate::failure::Failure;

pub(super) fn answer(context: &Context<'_>, call: &Call) -> Result<String, Failure> {
    let arguments = &call.arguments;
    let services = &*context.inner.services;
    let reading = &context.reading;
    match call.operation_name() {
        "find" => {
            let request = FindRequest {
                query: arguments.text_or_empty("name"),
                kind: arguments.text("kind").and_then(SymbolKind::from_name),
                limit: arguments.int_or("limit", DEFAULT_FIND_LIMIT),
            };
            context.viewed(app::find(services, reading, &request))
        }
        "overview" => Ok(overview(context, arguments)),
        "expand" => expand(context, arguments),
        "explain" => {
            let request = ExplainRequest {
                target: arguments.text_or_empty("target"),
                limit: arguments.int_or("limit", DEFAULT_EXPLAIN_LIMIT),
            };
            context.viewed(app::explain(services, reading, &request))
        }
        "locate" => {
            let request = LocsRequest {
                path: arguments.text_or_empty("path"),
                locs: arguments.texts("locations"),
                context: arguments.int_or("context_lines", DEFAULT_CONTEXT_LINES),
            };
            context.viewed(app::resolve_locations(reading, &request))
        }
        other => Err(Failure::Unplanned(format!(
            "symbols has no step for {other}"
        ))),
    }
}

// Each file keeps its block, and a file that failed shows the reason under its header.
fn overview(context: &Context<'_>, arguments: &Arguments) -> String {
    let request = SkeletonRequest {
        paths: arguments.texts("paths"),
        docstrings: arguments.switch("docstrings"),
        numbered: false,
    };
    let views = app::skeleton(&context.reading, &request);
    context.rendered(|receipt| tool_text(receipt, ToolForm::Overview(&views)))
}

// The ids that missed follow the bodies, on the one channel a tool answer has.
fn expand(context: &Context<'_>, arguments: &Arguments) -> Result<String, Failure> {
    let request = ExpandRequest {
        ids: arguments.texts("stable_ids"),
        limit: arguments.int_or("limit", DEFAULT_EXPAND_LIMIT),
    };
    let outcome = app::expand(&context.inner.services, &context.reading, &request);
    context.logged(&outcome.diagnostics);
    let result = outcome.value?;
    Ok(context.rendered(|receipt| tool_text(receipt, ToolForm::Expand(&result))))
}
