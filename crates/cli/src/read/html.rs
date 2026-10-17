//! The step of `html`: the interactive export on stdout, or written under the repository's cache
//! entry, with the receipt and what each bound left out on stderr.

use std::path::{Path, PathBuf};

use app::{HtmlGraph, HtmlRequest, Reading, Services, Session};
use render::{Receipt, html_document, html_summary, receipt_lines};

use super::{ReadArgs, open_reading};
use crate::Streams;
use crate::args::Arg;
use crate::exit::{EXIT_DOMAIN, EXIT_OK, EXIT_USAGE, code_for, fail, write_answer};
use crate::steps::{answer, diagnostics};

const MAX_CACHE_NAME: usize = 128;

pub(super) fn html(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let flags = args.flags();
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let cache_file = args.text("cache_file");
    let name = cache_file.and_then(cache_name);
    if cache_file.is_some() && name.is_none() {
        let message = format!(
            "--cache-file must be a simple .html name of at most {MAX_CACHE_NAME} characters"
        );
        return fail(streams.err, &message, EXIT_USAGE);
    }
    let request = HtmlRequest {
        max_nodes: args.int("max_nodes"),
        max_edges: args.int("max_edges"),
        resolution: args.float("resolution"),
    };
    let outcome = app::html(services, &Reading::of(&session), &request);
    diagnostics(streams, &outcome.diagnostics);
    match outcome.value {
        Ok(graph) => exported(&graph, name, &session, services, streams),
        Err(error) => fail(streams.err, &error.to_string(), code_for(&error)),
    }
}

// A name of letters, digits, dots, dashes and underscores that starts with a letter or a digit
// and ends in ".html": paths never cross this boundary.
fn cache_name(arg: &Arg) -> Option<&str> {
    let name = arg.raw.to_str()?;
    let stem = name.strip_suffix(".html")?;
    let first = stem.chars().next()?;
    let tail_allowed = |c: char| c.is_ascii_alphanumeric() || "._-".contains(c);
    let fits = name.chars().count() <= MAX_CACHE_NAME;
    (fits && first.is_ascii_alphanumeric() && stem.chars().all(tail_allowed)).then_some(name)
}

fn exported(
    graph: &HtmlGraph,
    name: Option<&str>,
    session: &Session,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let document = match html_document(graph) {
        Ok(document) => document,
        Err(error) => return answer(streams, Err(error), EXIT_DOMAIN),
    };
    let (out, mut err) = match name.map(|name| written(&document, name, session)) {
        None => (document, String::new()),
        Some(Ok(target)) => (
            String::new(),
            format!("agentless-mcp: wrote {}\n", target.display()),
        ),
        Some(Err(message)) => return fail(streams.err, &message, EXIT_DOMAIN),
    };
    let receipt = Receipt::of(session).counted_by(&*services.counter);
    err.push_str(&(receipt_lines(&receipt, &html_summary(graph)).join("\n") + "\n"));
    write_answer(streams, &out, &err, EXIT_OK)
}

// The export lands in the repository's own cache directory, under exports/, replaced whole.
fn written(document: &str, name: &str, session: &Session) -> Result<PathBuf, String> {
    let database = &session.cache.status.path;
    let directory = database.parent().unwrap_or(Path::new(""));
    let target = directory.join("exports").join(name);
    match io::replace_file(&target, document.as_bytes()) {
        Ok(()) => Ok(target),
        Err(error) => Err(format!(
            "cannot write HTML export: {}",
            io::os_error_text(&error, &target)
        )),
    }
}

#[cfg(test)]
mod tests;
