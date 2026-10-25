//! The steps of the symbol views: find-symbol and expand.

use app::{ExpandRequest, ExpandResult, FindRequest, FindResult, Reading, Services};
use facts::SymbolKind;
use render::expand_stderr_rows;

use super::{Answering, ReadArgs, emit_then, open_reading, utf8_path};
use crate::Streams;
use crate::exit::{EXIT_DOMAIN, EXIT_OK};

pub(super) fn find(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let name = args.text("name").expect("the parser requires NAME");
    let (query, flags) = match utf8_path(name, "NAME", streams) {
        Ok(query) => (query, args.flags()),
        Err(code) => return code,
    };
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let kind = args.text("kind");
    let request = FindRequest {
        query,
        kind: kind.and_then(|kind| SymbolKind::from_name(&kind.text)),
        limit: args.int("limit"),
    };
    let outcome = app::find(services, &Reading::of(&session), &request);
    let answering = Answering {
        session: &session,
        services,
        flags: &flags,
    };
    emit_then(outcome, &answering, streams, find_trailer)
}

// A query that matches no symbol and that no code file spells fails the call.
fn find_trailer(result: &FindResult) -> (String, u8) {
    let code = if result.is_miss() {
        EXIT_DOMAIN
    } else {
        EXIT_OK
    };
    (String::new(), code)
}

pub(super) fn expand(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let ids: Result<Vec<String>, u8> = (args.list("ids").iter())
        .map(|arg| utf8_path(arg, "STABLE_ID", streams))
        .collect();
    let (ids, flags) = match ids {
        Ok(ids) => (ids, args.flags()),
        Err(code) => return code,
    };
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let request = ExpandRequest {
        ids,
        limit: args.int("limit"),
    };
    let outcome = app::expand(services, &Reading::of(&session), &request);
    let answering = Answering {
        session: &session,
        services,
        flags: &flags,
    };
    emit_then(outcome, &answering, streams, expand_trailer)
}

// Each id that did not resolve is a row on stderr, and any one of them fails the call.
fn expand_trailer(result: &ExpandResult) -> (String, u8) {
    let rows: Vec<String> = expand_stderr_rows(result)
        .iter()
        .map(|row| note(&format!("agentless-mcp: {row}")))
        .collect();
    let code = if result.unresolved.is_empty() {
        EXIT_OK
    } else {
        EXIT_DOMAIN
    };
    (rows.concat(), code)
}

// A stderr note ends with one newline, and gains none when it has one.
fn note(text: &str) -> String {
    if text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    }
}
