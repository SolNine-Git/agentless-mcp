//! The step of `map`: the ranked, token-budgeted repository map.

use app::{IntArg, MapRequest, Reading, Services};
use io::{Granularity, ProjectConfig};

use super::body::map_body;
use super::{ReadArgs, emit, open_reading};
use crate::Streams;
use crate::exit::{EXIT_USAGE, fail};

const AUTO_BUDGET: &str = "auto";

pub(super) fn map(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let flags = args.flags();
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let raw = args.text("budget").map(|arg| arg.text.as_str());
    let budget = match budget(raw, &session.repo.config) {
        Ok(budget) => budget,
        Err(refusal) => return fail(streams.err, &refusal, EXIT_USAGE),
    };
    let granularity = match args.text("granularity").map(|arg| arg.text.as_str()) {
        Some("body") => return map_body(args, &session, budget, services, streams),
        Some("file") => Some(Granularity::File),
        Some(_) => Some(Granularity::Function),
        None => None,
    };
    let request = MapRequest {
        focus: args
            .list("focus")
            .iter()
            .map(|arg| arg.text.clone())
            .collect(),
        budget,
        max_files: args.given_int("max_files"),
        granularity,
    };
    let outcome = app::map(services, &Reading::of(&session), &request);
    emit(outcome, &session, services, &flags, streams)
}

// `None` is the auto size. An unset --budget takes the config's budget, so "auto" and "nothing
// said" are two decisions here.
fn budget(raw: Option<&str>, config: &ProjectConfig) -> Result<Option<IntArg>, String> {
    let Some(raw) = raw else {
        return Ok(config
            .map_budget
            .map(|budget| IntArg::from_usize(budget as usize)));
    };
    if raw == AUTO_BUDGET {
        return Ok(None);
    }
    let parsed = IntArg::parse(raw).filter(|value| value.value().is_some_and(|number| number > 0));
    let refusal = || format!("--budget takes a positive integer or '{AUTO_BUDGET}'");
    parsed.map(Some).ok_or_else(refusal)
}
