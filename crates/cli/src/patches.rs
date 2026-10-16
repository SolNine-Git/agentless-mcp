//! The `patch` steps: parse, and check, apply and normalize against a repository. The answer
//! goes to stdout; the receipt, the summary and each edit that did not apply go to stderr.

use std::io::Read;
use std::path::Path;

use app::{ApplyMode, ApplyReport, ApplyResult, CheckReport, Edit, NormalizeReport, Services};
use serde::Serialize;

use crate::Streams;
use crate::caller::{named_file, shown_path, stdin_text};
use crate::exit::{EXIT_USAGE, code_for, domain_code, fail};
use crate::invocation::{EditStep, PatchEdit, PatchInput};
use crate::steps::{Resolved, answer_with, diagnostics_or_stop, resolved};

// The repository and the edits of one check, apply or normalize, and the form of its answer.
struct Prepared {
    found: Resolved,
    edits: Vec<Edit>,
    json: bool,
}

pub(crate) fn parse(
    input: &PatchInput,
    services: &Services,
    stdin: &mut dyn Read,
    streams: &mut Streams<'_>,
) -> u8 {
    let text = match patch_text(input, services, stdin) {
        Ok(text) => text,
        Err(refusal) => return fail(streams.err, &refusal, EXIT_USAGE),
    };
    let parsed = app::parse_blocks(&text);
    let ok = parsed.errors.is_empty();
    let notes = if ok {
        String::new()
    } else {
        render::parse_errors(&parsed.errors)
    };
    answer_with(
        streams,
        render::document_json(&parsed),
        &notes,
        domain_code(ok),
    )
}

// The repository first, then the patch, then the step.
pub(crate) fn edit(
    call: &PatchEdit,
    services: &Services,
    stdin: &mut dyn Read,
    streams: &mut Streams<'_>,
) -> u8 {
    let found = match resolved(call.flags.repo.as_deref(), app::resolve, services, streams) {
        Ok(found) => found,
        Err(code) => return code,
    };
    let text = match patch_text(&call.input, services, stdin) {
        Ok(text) => text,
        Err(refusal) => return fail(streams.err, &refusal, EXIT_USAGE),
    };
    let edits = match edits_of(&text) {
        Ok(edits) => edits,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let prepared = Prepared {
        found,
        edits,
        json: call.flags.json,
    };
    let (repo, edits) = (&prepared.found.repo, &prepared.edits);
    match call.step {
        EditStep::Check => answered(&prepared, app::check_patch(repo, edits), streams),
        EditStep::Apply(mode) => applied(&prepared, mode, services, streams),
        EditStep::Normalize => answered(&prepared, app::normalize_patch(repo, edits), streams),
    }
}

// The diagnostics come with either outcome; a SIGINT that the write held ends the run after them.
fn applied(
    prepared: &Prepared,
    mode: ApplyMode,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let found = &prepared.found;
    let noted = app::apply_patch(
        services,
        &found.repo,
        &found.snapshot,
        &prepared.edits,
        mode,
    );
    diagnostics_or_stop(streams, &noted.diagnostics);
    answered(prepared, noted.value, streams)
}

// What the answer of a step reads from its report.
trait StepReport: Serialize {
    fn text(&self) -> String;
    fn summary(&self) -> String;
    fn result(&self) -> &ApplyResult;
    fn ok(&self) -> bool;
}

// The three reports name their summary, outcomes and verdict alike; only the text differs.
macro_rules! step_reports {
    ($($report:ident => $text:expr),*) => {$(
        impl StepReport for $report {
            fn text(&self) -> String {
                let text: fn(&$report) -> String = $text;
                text(self)
            }

            fn summary(&self) -> String {
                self.summary_line()
            }

            fn result(&self) -> &ApplyResult {
                &self.result
            }

            fn ok(&self) -> bool {
                $report::ok(self)
            }
        }
    )*};
}

// An empty diff prints nothing.
step_reports!(
    CheckReport => render::check_text,
    ApplyReport => |report| line_ended(&report.diff),
    NormalizeReport => |report| line_ended(&report.key)
);

// The JSON document or the text on stdout; the receipt with the summary and the edits that did
// not apply on stderr. A step that did not apply every edit exits 1.
fn answered<R: StepReport>(
    prepared: &Prepared,
    report: Result<R, app::Error>,
    streams: &mut Streams<'_>,
) -> u8 {
    let report = match report {
        Ok(report) => report,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let out = if prepared.json {
        render::document_json(&report)
    } else {
        Ok(report.text())
    };
    let receipt = prepared.found.receipt();
    let notes = render::patch_receipt(&receipt, &report.summary(), report.result());
    answer_with(streams, out, &notes, domain_code(report.ok()))
}

// The text with a final line break, and nothing for an empty text.
fn line_ended(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    }
}

// The file that -f names, or stdin without -f and for "-".
fn patch_text(
    input: &PatchInput,
    services: &Services,
    stdin: &mut dyn Read,
) -> Result<String, String> {
    match &input.file {
        Some(file) if file.text != "-" => {
            named_file(Path::new(&file.raw), &shown_path(&file.text), services)
        }
        _ => stdin_text(stdin),
    }
}

// A patch with any block that did not parse is refused whole, so a truncated patch applies
// nothing.
fn edits_of(text: &str) -> Result<Vec<Edit>, app::Error> {
    let parsed = app::load_edits(text)?;
    if parsed.errors.is_empty() {
        return Ok(parsed.edits);
    }
    let blocks: Vec<String> = parsed.errors.iter().map(render::block_line).collect();
    let message = format!(
        "{} of {} blocks did not parse, so none of them was applied:\n{}",
        parsed.errors.len(),
        parsed.errors.len() + parsed.edits.len(),
        blocks.join("\n")
    );
    Err(kernel::Error::OperationFailed(message).into())
}
