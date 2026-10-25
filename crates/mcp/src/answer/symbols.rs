//! The symbols operations: find, overview, expand, explain and locate.

use app::ops::limits::{
    DEFAULT_CONTEXT_LINES, DEFAULT_EXPAND_LIMIT, DEFAULT_EXPLAIN_LIMIT, DEFAULT_FIND_LIMIT,
};
use app::{ExpandRequest, ExplainRequest, FileView, FindRequest, LocsRequest, SkeletonRequest};
use facts::SymbolKind;
use render::{ToolForm, expand_stderr_rows, one_line, tool_text};

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
        "overview" => overview(context, arguments),
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

// Each file keeps its block, and a file that failed shows the reason under its header. An overview
// whose every file failed fails with the reasons.
fn overview(context: &Context<'_>, arguments: &Arguments) -> Result<String, Failure> {
    let request = SkeletonRequest {
        paths: arguments.texts("paths"),
        docstrings: arguments.switch("docstrings"),
        numbered: false,
    };
    let views = app::skeleton(&context.reading, &request);
    if let Some(reasons) = overview_failure(&views) {
        return Err(Failure::Refused(reasons));
    }
    Ok(context.rendered(|receipt| tool_text(receipt, ToolForm::Overview(&views))))
}

// The reason of each file, one per line, when no file gave an overview.
fn overview_failure(views: &[FileView]) -> Option<String> {
    let failed = |view: &FileView| !view.error.is_empty();
    if views.is_empty() || !views.iter().all(failed) {
        return None;
    }
    let reasons: Vec<_> = views.iter().map(|view| one_line(&view.error)).collect();
    Some(reasons.join("\n"))
}

// The ids that missed follow the bodies, on the one channel a tool answer has. An expansion where
// no id resolved fails with those rows.
fn expand(context: &Context<'_>, arguments: &Arguments) -> Result<String, Failure> {
    let request = ExpandRequest {
        ids: arguments.texts("stable_ids"),
        limit: arguments.int_or("limit", DEFAULT_EXPAND_LIMIT),
    };
    let outcome = app::expand(&context.inner.services, &context.reading, &request);
    context.logged(&outcome.diagnostics);
    let result = outcome.value?;
    if result.cards.is_empty() && result.unresolved_total() > 0 {
        return Err(Failure::Refused(expand_stderr_rows(&result).join("\n")));
    }
    Ok(context.rendered(|receipt| tool_text(receipt, ToolForm::Expand(&result))))
}

#[cfg(test)]
mod tests;
