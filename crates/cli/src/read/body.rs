//! The step of `map --granularity body`: the map's file rows and its best symbols' bodies.

use app::{IntArg, MapRequest, Reading, Services, Session};

use super::{ReadArgs, emit};
use crate::Streams;

// The map step has opened the session and checked the budget.
pub(super) fn map_body(
    args: &ReadArgs<'_>,
    session: &Session,
    budget: Option<IntArg>,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let request = MapRequest {
        focus: args
            .list("focus")
            .iter()
            .map(|arg| arg.text.clone())
            .collect(),
        budget,
        max_files: args.given_int("max_files"),
        granularity: None,
    };
    let outcome = app::body_map(services, &Reading::of(session), &request);
    emit(outcome, session, services, &args.flags(), streams)
}
