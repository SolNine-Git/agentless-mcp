//! The step of `diagram`: bare mermaid on stdout for a document, or its JSON, with the receipt and
//! the caveat on stderr; or a byte comparison against a committed diagram.

use std::path::Path;

use app::{DiagramRequest, DiagramView, Reading, Services, Session};
use render::{Receipt, diagram_caveat, diagram_summary, mermaid, receipt_lines, strip_fence};

use super::{ReadArgs, open_reading, utf8_path};
use crate::Streams;
use crate::caller::{named_file, shown_path};
use crate::exit::{EXIT_DOMAIN, EXIT_OK, EXIT_USAGE, code_for, fail, report, write_answer};
use crate::steps::{answer, diagnostics};

pub(super) fn diagram(args: &ReadArgs<'_>, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let ([focus, check], flags) = match texts(args, streams) {
        Ok(texts) => (texts, args.flags()),
        Err(code) => return code,
    };
    let session = match open_reading(&flags, services, streams) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let request = DiagramRequest {
        focus,
        max_nodes: args.int("max_nodes"),
        max_edges: args.int("max_edges"),
        group_by_communities: args.flag("communities"),
        resolution: args.float("resolution"),
    };
    let outcome = app::diagram(services, &Reading::of(&session), &request);
    diagnostics(streams, &outcome.diagnostics);
    let view = match outcome.value {
        Ok(view) => view,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    if !view.message.is_empty() {
        return fail(streams.err, &view.message, EXIT_DOMAIN);
    }
    match check {
        Some(check) => check_diagram(&mermaid(&view), &check, services, streams),
        None => drawn(&view, &session, services, flags.json, streams),
    }
}

// The focus and the --check file, in that order; the first that is not UTF-8 is refused.
fn texts(args: &ReadArgs<'_>, streams: &mut Streams<'_>) -> Result<[Option<String>; 2], u8> {
    let mut texts = [None, None];
    let named = [("focus", "FILE_OR_SYMBOL"), ("check", "FILE")];
    for (text, (dest, name)) in texts.iter_mut().zip(named) {
        let given = args.text(dest).map(|arg| utf8_path(arg, name, streams));
        *text = given.transpose()?;
    }
    Ok(texts)
}

// The diagram goes to stdout untouched, so a caller can paste it; what describes the run goes to
// stderr.
fn drawn(
    view: &DiagramView,
    session: &Session,
    services: &Services,
    json: bool,
    streams: &mut Streams<'_>,
) -> u8 {
    let receipt = Receipt::of(session).counted_by(&*services.counter);
    let rendered = if json {
        render::json(&receipt, view)
    } else {
        Ok(mermaid(view))
    };
    let mut lines = receipt_lines(&receipt, &diagram_summary(view));
    lines.extend(diagram_caveat(view).map(|caveat| format!("agentless-mcp: {caveat}")));
    let err = lines.join("\n") + "\n";
    match rendered {
        Ok(out) => write_answer(streams, &out, &err, EXIT_OK),
        Err(error) => answer(streams, Err(error), EXIT_DOMAIN),
    }
}

// Byte-exact after a fence that opens and closes the file is stripped. A drift names the first
// line that differs and the two lengths.
fn check_diagram(
    rendered: &str,
    check: &str,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let shown = shown_path(check);
    let committed = match named_file(Path::new(check), &shown, services) {
        Ok(text) => text,
        Err(refusal) => return fail(streams.err, &refusal, EXIT_USAGE),
    };
    let stripped = strip_fence(&committed);
    if stripped == rendered {
        report(
            streams.err,
            &format!("agentless-mcp: {shown} matches the current diagram\n"),
        );
        return EXIT_OK;
    }
    let mut lines = vec![format!(
        "agentless-mcp: {shown} has drifted from the current diagram"
    )];
    let drift = drift_summary(&stripped, rendered);
    lines.extend(drift.into_iter().map(|line| format!("  {line}")));
    report(streams.err, &(lines.join("\n") + "\n"));
    EXIT_DOMAIN
}

fn drift_summary(committed: &str, rendered: &str) -> Vec<String> {
    let old: Vec<&str> = committed.split('\n').collect();
    let new: Vec<&str> = rendered.split('\n').collect();
    let mut lines = vec![format!(
        "committed {} lines, regenerated {} lines",
        old.len(),
        new.len()
    )];
    let first = old
        .iter()
        .zip(&new)
        .position(|(before, after)| before != after);
    match first {
        Some(at) => lines.extend([
            format!("first difference at line {}:", at + 1),
            format!("  committed:   {}", old[at]),
            format!("  regenerated: {}", new[at]),
        ]),
        None => lines.push(format!(
            "identical for the first {} lines, then one is longer",
            old.len().min(new.len())
        )),
    }
    lines
}

#[cfg(test)]
mod tests;
