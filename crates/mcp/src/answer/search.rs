//! The `search` tool: the match lines of a pattern, or the files that a glob lists.

use app::search::{DEFAULT_SEARCH_LIMIT, SearchError, SearchRequest};
use render::{SearchAnswer, search_refusal};

use super::Context;
use crate::arguments::Arguments;
use crate::failure::Failure;

pub(super) fn answer(context: &Context<'_>, arguments: &Arguments) -> Result<String, Failure> {
    let text = |name: &str| arguments.text(name).map(str::to_owned);
    let request = SearchRequest {
        pattern: text("pattern"),
        files: text("files"),
        path: text("path"),
        ignore_case: arguments.flag("ignore_case"),
        context: arguments.int_or("context", 0),
        limit: arguments.int_or("limit", DEFAULT_SEARCH_LIMIT),
    };
    let outcome = app::search::search(&context.inner.services, &context.reading, &request);
    context.logged(&outcome.diagnostics);
    let searched = outcome.value.map_err(refused)?;
    Ok(context.rendered(&SearchAnswer {
        request: &request,
        searched: &searched,
    }))
}

// A refusal of the bounds, the containment, the walk or the deadline keeps the server's own form.
fn refused(error: SearchError) -> Failure {
    match error {
        SearchError::Refused(error) => Failure::from(error),
        other => Failure::Refused(search_refusal(&other)),
    }
}
