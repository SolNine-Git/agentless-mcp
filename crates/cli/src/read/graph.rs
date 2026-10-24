//! The steps of the graph views: explain, path, communities and health.

use app::{
    CommunitiesRequest, Error, ExplainRequest, HealthRequest, Noted, PathPolicy, PathRequest,
    Reading, Services,
};
use render::View;

use super::{Answering, ReadArgs, emit, emit_then, open_reading, utf8_path};
use crate::Streams;
use crate::exit::{EXIT_DOMAIN, EXIT_OK};

// A target that matches nothing is answered, and exits 1.
pub(super) fn explain(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let target = args.text("target").expect("the parser requires the target");
    let (target, flags) = match utf8_path(target, "NAME_OR_STABLE_ID", streams) {
        Ok(target) => (target, args.flags()),
        Err(code) => return code,
    };
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let request = ExplainRequest {
        target,
        limit: args.int("limit"),
    };
    let outcome = app::explain(services, &Reading::of(&session), &request);
    let answering = Answering {
        session: &session,
        services,
        flags: &flags,
    };
    emit_then(outcome, &answering, streams, |explained| {
        (String::new(), exit_code(explained.card.is_some()))
    })
}

// An endpoint that names nothing exits 1; two endpoints with no path between them exit 0.
pub(super) fn path(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let ends = ["source", "target"].map(|dest| args.text(dest).expect("the parser requires both"));
    let source = utf8_path(ends[0], "FROM", streams);
    let target = source.and_then(|source| Ok((source, utf8_path(ends[1], "TO", streams)?)));
    let ((source, target), flags) = match target {
        Ok(ends) => (ends, args.flags()),
        Err(code) => return code,
    };
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let request = PathRequest {
        source,
        target,
        policy: PathPolicy {
            include_unique: args.flag("include_unique"),
            include_ambiguous: args.flag("include_ambiguous"),
        },
        max_visited: args.int("max_visited"),
    };
    let outcome = app::path(services, &Reading::of(&session), &request);
    let answering = Answering {
        session: &session,
        services,
        flags: &flags,
    };
    emit_then(outcome, &answering, streams, |trace| {
        (String::new(), exit_code(trace.endpoints_resolved))
    })
}

pub(super) fn communities(
    args: &ReadArgs<'_>,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let request = CommunitiesRequest {
        resolution: args.float("resolution"),
        limit: args.int("limit"),
        members: args.int("members"),
    };
    answered(args, services, streams, |reading| {
        app::communities(services, reading, &request)
    })
}

pub(super) fn health(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let request = HealthRequest {
        limit: args.int("limit"),
    };
    answered(args, services, streams, |reading| {
        app::health(services, reading, &request)
    })
}

fn exit_code(answered: bool) -> u8 {
    if answered { EXIT_OK } else { EXIT_DOMAIN }
}

// Opens the session, runs `operation` on it, and prints its answer with exit 0.
fn answered<V: View>(
    args: &ReadArgs<'_>,
    services: &Services,
    streams: &mut Streams<'_>,
    operation: impl FnOnce(&Reading<'_>) -> Noted<Result<V, Error>>,
) -> u8 {
    let flags = args.flags();
    match open_reading(&flags, services, streams) {
        Ok(session) => {
            let outcome = operation(&Reading::of(&session));
            emit(outcome, &session, services, &flags, streams)
        }
        Err(code) => code,
    }
}
