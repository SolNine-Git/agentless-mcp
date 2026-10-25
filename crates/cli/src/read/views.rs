//! The steps of the file and directory views: the tree, skeletons, slices and locations.

use std::str::FromStr;

use app::{
    FileView, IntArg, LocateRequest, LocationView, LocsRequest, Reading, Services, Session,
    SkeletonRequest, SliceRequest, TreeRequest,
};
use facts::StableId;

use super::{Answering, ReadArgs, emit, emit_then, open_reading, utf8_path};
use crate::Streams;
use crate::exit::{EXIT_DOMAIN, EXIT_OK, EXIT_USAGE, fail};

const SLICE_NEEDS_SOURCE: &str = "slice needs a FILE or --symbol STABLE_ID";
const LINES_SHAPE: &str = "--lines takes A:B with 1-based inclusive integers";
const LINES_WITH_SYMBOL: &str = "slice takes --lines with FILE, not with --symbol";

pub(super) fn tree(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let flags = args.flags();
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let request = TreeRequest {
        depth: args.int("depth"),
        max_entries: args.int("max_entries"),
    };
    let outcome = app::tree(services, &session.repo, &request);
    emit(outcome, &session, services, &flags, streams)
}

// A failed file goes to stderr, never into the answer: a refused path exits 2, an unreadable one 1.
pub(super) fn skeleton(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let paths: Result<Vec<String>, u8> = (args.list("files").iter())
        .map(|arg| utf8_path(arg, "FILE", streams))
        .collect();
    let (paths, flags) = match paths {
        Ok(paths) => (paths, args.flags()),
        Err(code) => return code,
    };
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let request = SkeletonRequest {
        paths,
        docstrings: args.switch("docstrings"),
        numbered: args.flag("numbers"),
    };
    let views = app::skeleton(&Reading::of(&session), &request);
    let answering = Answering {
        session: &session,
        services,
        flags: &flags,
    };
    emit_then(app::Noted::new(Ok(views)), &answering, streams, |views| {
        skeleton_trailer(views)
    })
}

fn skeleton_trailer(views: &[FileView]) -> (String, u8) {
    let failed = views.iter().filter(|view| !view.error.is_empty());
    let rows: Vec<String> = failed
        .map(|view| format!("agentless-mcp: {}\n", view.error))
        .collect();
    let rows = rows.concat();
    let code = if views.iter().any(|view| view.refused) {
        EXIT_USAGE
    } else if rows.is_empty() {
        EXIT_OK
    } else {
        EXIT_DOMAIN
    };
    (rows, code)
}

pub(super) fn slice(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let flags = args.flags();
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let answering = Answering {
        session: &session,
        services,
        flags: &flags,
    };
    if let Some(symbol) = args.text("symbol") {
        return by_symbol(args, &symbol.text, &answering, streams);
    }
    let Some(file) = args.text("file") else {
        return fail(streams.err, SLICE_NEEDS_SOURCE, EXIT_USAGE);
    };
    let path = match utf8_path(file, "FILE", streams) {
        Ok(path) => path,
        Err(code) => return code,
    };
    let Some(ranges) = ranges(args) else {
        return fail(streams.err, LINES_SHAPE, EXIT_USAGE);
    };
    let request = SliceRequest {
        path,
        ranges,
        context: args.int("context"),
    };
    file_slice(&session, &request, &answering, streams)
}

// A slice that gives no line fails the run with one row per reason: the file cannot be read, or
// each range starts past its end. Any other slice is an answer.
fn file_slice(
    session: &Session,
    request: &SliceRequest,
    answering: &Answering<'_>,
    streams: &mut Streams<'_>,
) -> u8 {
    let outcome = app::slice(&Reading::of(session), request);
    let failure = match &outcome.value {
        Ok(view) if !view.error.is_empty() => Some(view.error.clone()),
        _ => None,
    };
    if let Some(reasons) = failure {
        crate::steps::diagnostics(streams, &outcome.diagnostics);
        for reason in reasons.split('\n') {
            fail(streams.err, reason, EXIT_DOMAIN);
        }
        return EXIT_DOMAIN;
    }
    emit_then(outcome, answering, streams, |_| (String::new(), EXIT_OK))
}

// The `--lines` values as `A:B` ranges, or `B` alone as `B:B`; `None` for the first one that is
// not two integers with 1 <= A <= B.
fn ranges(args: &ReadArgs<'_>) -> Option<Vec<(usize, usize)>> {
    args.list("lines")
        .iter()
        .map(|arg| {
            let (head, tail) = arg.text.split_once(':').unwrap_or((&arg.text, &arg.text));
            let start = IntArg::parse(head)?.value()?;
            let end = IntArg::parse(tail)?.value()?;
            (start >= 1 && end >= start).then_some((start, end))
        })
        .collect()
}

// A stable id resolves through the location forms its name can take; an id that names nothing
// exits 1 with every form's miss.
fn by_symbol(
    args: &ReadArgs<'_>,
    symbol: &str,
    answering: &Answering<'_>,
    streams: &mut Streams<'_>,
) -> u8 {
    if !args.list("lines").is_empty() {
        return fail(streams.err, LINES_WITH_SYMBOL, EXIT_USAGE);
    }
    let id = match StableId::from_str(symbol) {
        Ok(id) => id,
        Err(error) => return fail(streams.err, &error.to_string(), EXIT_USAGE),
    };
    let request = LocateRequest {
        id,
        context: args.int("context"),
    };
    let outcome = app::locate_symbol(&Reading::of(answering.session), &request);
    emit_then(outcome, answering, streams, |view| {
        (String::new(), located(view))
    })
}

fn located(view: &LocationView) -> u8 {
    if view.resolution.stable_ids.is_empty() {
        EXIT_DOMAIN
    } else {
        EXIT_OK
    }
}

pub(super) fn resolve_locs(
    args: &ReadArgs<'_>,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let file = args.text("file").expect("the parser requires FILE");
    let path = match utf8_path(file, "FILE", streams) {
        Ok(path) => path,
        Err(code) => return code,
    };
    let flags = args.flags();
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let request = LocsRequest {
        path,
        locs: args
            .list("loc")
            .iter()
            .map(|arg| arg.text.clone())
            .collect(),
        context: args.int("context"),
    };
    let outcome = app::resolve_locations(&Reading::of(&session), &request);
    emit(outcome, &session, services, &flags, streams)
}
