//! The tools without an operation table: `find_referencing_symbols`, `history` and
//! `capabilities`.

use app::ops::limits::{DEFAULT_HISTORY_BUDGET, DEFAULT_HISTORY_LIMIT, DEFAULT_REFS_LIMIT};
use app::{BindingStatus, HistoryRequest, RefsRequest, Roots};
use render::one_line;

use super::Context;
use crate::arguments::Arguments;
use crate::failure::Failure;

// A target that binds to nothing gives no data, so its line is a refusal.
pub(super) fn refs(context: &Context<'_>, arguments: &Arguments) -> Result<String, Failure> {
    let request = RefsRequest {
        target: arguments.required("target").to_owned(),
        limit: arguments.int_or("limit", DEFAULT_REFS_LIMIT),
        shared_callers: arguments.flag("shared_callers"),
    };
    let services = &*context.inner.services;
    let outcome = app::refs(services, &context.reading, &request);
    if let Ok(result) = &outcome.value
        && result.binding.status == BindingStatus::NotFound
    {
        context.logged(&outcome.diagnostics);
        return Err(Failure::Refused(
            one_line(&result.binding.message).into_owned(),
        ));
    }
    context.viewed(outcome)
}

pub(super) fn history(context: &Context<'_>, arguments: &Arguments) -> Result<String, Failure> {
    let request = HistoryRequest {
        target: arguments.required("target").to_owned(),
        limit: arguments.int_or("limit", DEFAULT_HISTORY_LIMIT),
        budget: arguments.int_or("budget", DEFAULT_HISTORY_BUDGET),
    };
    let services = &*context.inner.services;
    context.viewed(app::history(services, &context.reading, &request))
}

// The report names this server's roots; it never asks the client for its own. Its cache receipt
// reads as of now, as the answer's receipt does.
pub(super) fn capabilities(context: &Context<'_>) -> Result<String, Failure> {
    let mut report = app::capabilities(context.session)?;
    report.cache.receipt = context.cache_field();
    report.roots = Roots {
        configured: Some(
            context
                .allowed
                .iter()
                .map(|root| root.display().to_string())
                .collect(),
        ),
        client: Vec::new(),
    };
    Ok(context.rendered(|receipt| render::capabilities_text(receipt, &report)))
}
