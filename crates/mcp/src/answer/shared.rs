//! The tools without an operation table: `find_referencing_symbols`, `history` and
//! `capabilities`.

use app::ops::limits::{DEFAULT_HISTORY_BUDGET, DEFAULT_HISTORY_LIMIT, DEFAULT_REFS_LIMIT};
use app::{HistoryRequest, RefsRequest, Roots, Session};

use super::Context;
use crate::arguments::Arguments;
use crate::failure::Failure;

pub(super) fn refs(context: &Context<'_>, arguments: &Arguments) -> Result<String, Failure> {
    let request = RefsRequest {
        target: arguments.required("target").to_owned(),
        limit: arguments.int_or("limit", DEFAULT_REFS_LIMIT),
        shared_callers: arguments.flag("shared_callers"),
    };
    let services = &*context.inner.services;
    context.viewed(app::refs(services, &context.reading, &request))
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

// The report names this server's roots; it never asks the client for its own.
pub(super) fn capabilities(context: &Context<'_>, session: &Session) -> Result<String, Failure> {
    let mut report = app::capabilities(session)?;
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
    Ok(render::capabilities_text(&context.receipt, &report))
}
