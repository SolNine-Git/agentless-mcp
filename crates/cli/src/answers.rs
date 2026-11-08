//! The steps of `trace` and `search`: the answers of the tools of the same names, under the
//! receipt, as text or with `--json`.

use app::search::{DEFAULT_SEARCH_LIMIT, SearchError, SearchRequest};
use app::trace::{Section, TraceRequest};
use app::{IntArg, Noted, Reading, Services};
use render::{SearchAnswer, TraceAnswer, search_refusal};

use crate::Streams;
use crate::args::{Namespace, Value};
use crate::exit::{EXIT_USAGE, fail, failed};
use crate::invocation::{Answer, AnswerCall, repo_flags};
use crate::read::{emit, open_reading};
use crate::steps::diagnostics;

pub(crate) fn run(call: &AnswerCall, services: &Services, streams: &mut Streams<'_>) -> u8 {
    match call.answer {
        Answer::Trace => trace(&call.namespace, services, streams),
        Answer::Search => search(&call.namespace, services, streams),
    }
}

fn trace(namespace: &Namespace, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let request = match trace_request(namespace) {
        Ok(request) => request,
        Err(refusal) => return fail(streams.err, &refusal, EXIT_USAGE),
    };
    let flags = repo_flags(namespace);
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let Noted {
        value,
        diagnostics: notes,
    } = app::trace::trace(services, &Reading::of(&session), &request);
    let trace = match value {
        Ok(trace) => trace,
        Err(error) => {
            diagnostics(streams, &notes);
            return failed(streams.err, &error);
        }
    };
    let view = TraceAnswer {
        request: &request,
        trace: &trace,
    };
    let outcome = Noted {
        value: Ok(view),
        diagnostics: notes,
    };
    emit(outcome, &session, services, &flags, streams)
}

fn search(namespace: &Namespace, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let request = match search_request(namespace) {
        Ok(request) => request,
        Err(refusal) => return fail(streams.err, &refusal, EXIT_USAGE),
    };
    let flags = repo_flags(namespace);
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let Noted {
        value,
        diagnostics: notes,
    } = app::search::search(services, &Reading::of(&session), &request);
    let searched = match value {
        Ok(searched) => searched,
        Err(error) => {
            diagnostics(streams, &notes);
            return search_failed(&error, streams);
        }
    };
    let view = SearchAnswer {
        request: &request,
        searched: &searched,
    };
    let outcome = Noted {
        value: Ok(view),
        diagnostics: notes,
    };
    emit(outcome, &session, services, &flags, streams)
}

// The tools take each target as text, so a target that is not UTF-8 is refused.
fn trace_request(namespace: &Namespace) -> Result<TraceRequest, String> {
    let targets = (namespace.list("target").iter())
        .map(|target| target.utf8("TARGET"))
        .collect::<Result<_, _>>()?;
    Ok(TraceRequest {
        targets,
        more: (namespace.text("more")).and_then(|arg| Section::from_name(&arg.text)),
    })
}

fn search_request(namespace: &Namespace) -> Result<SearchRequest, String> {
    let text = |dest: &str, name: &str| namespace.text(dest).map(|arg| arg.utf8(name)).transpose();
    let int = |dest: &str, default: usize| match namespace.get(dest) {
        Some(Value::Int(value)) => value.clone(),
        _ => IntArg::from_usize(default),
    };
    Ok(SearchRequest {
        pattern: text("pattern", "PATTERN")?,
        files: text("files", "--files")?,
        path: text("path", "--path")?,
        ignore_case: namespace.flag("ignore_case"),
        context: int("context", 0),
        limit: int("limit", DEFAULT_SEARCH_LIMIT),
    })
}

// A refusal of the bounds, the containment, the walk or the deadline keeps the command line's own
// form; every other refusal of the request exits 2.
fn search_failed(error: &SearchError, streams: &mut Streams<'_>) -> u8 {
    match error {
        SearchError::Refused(error) => failed(streams.err, error),
        other => fail(streams.err, &search_refusal(other), EXIT_USAGE),
    }
}
