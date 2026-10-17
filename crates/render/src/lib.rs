//! The text and JSON forms of the answers: the receipt envelope, the lint report, the
//! capabilities report, the read views and their tool forms, and the patch, validate and vote
//! answers. Every function returns the bytes to print and does no I/O.

mod blocks;
mod capabilities;
mod communities;
mod count;
mod cycles;
mod envelope;
mod explain;
mod health;
mod history;
mod html;
mod index;
mod lint;
mod map;
mod map_body;
mod mermaid;
mod patches;
mod path;
mod receipt;
mod refs;
mod skipped;
mod symbols;
#[cfg(test)]
mod tests;
mod tool;
mod tree;
mod view;
mod views;
mod vote;
mod warmup;

use app::{Capabilities, Chars4, LintReport, ValidateReport};
use json::JsonForm;
use serde::Serialize;

use crate::count::Ceiling;

pub use html::{html_document, html_summary};
pub use index::{index_json, index_text};
pub use mermaid::{diagram_caveat, diagram_summary, mermaid, strip_fence};
pub use patches::{block_line, check_text, note_text, parse_errors, patch_receipt};
pub use receipt::{Receipt, receipt_lines};
pub use symbols::expand_stderr_rows;
pub use text::one_line;
pub use tool::{ToolForm, tool_text};
pub use view::{Rendered, Truncation, View, json, text};
pub use vote::{vote_summary, vote_text};
pub use warmup::warmup_text;

/// The JSON form of an answer could not be written.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The serializer refused a value, for example a path that is not UTF-8.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Returns the lint report as text under its receipt.
#[must_use]
pub fn lint_text(receipt: &Receipt<'_>, report: &LintReport) -> String {
    let body = lint::render_lint(report);
    envelope::wrap(receipt, &body, &receipt.ceiling(), None)
}

/// Returns the lint report as a JSON document that starts with its receipt.
///
/// # Errors
///
/// Returns the serializer's error for a value it cannot write.
pub fn lint_json(receipt: &Receipt<'_>, report: &LintReport) -> Result<String, Error> {
    envelope::wrap_json(receipt, report, &receipt.ceiling())
}

/// Returns the capabilities report as text under its receipt.
#[must_use]
pub fn capabilities_text(receipt: &Receipt<'_>, report: &Capabilities<'_>) -> String {
    let body = capabilities::render_capabilities(report);
    envelope::wrap(receipt, &body, &receipt.ceiling(), None)
}

/// Returns the capabilities report as a JSON document that starts with its receipt.
///
/// # Errors
///
/// Returns the serializer's error for a value it cannot write.
pub fn capabilities_json(
    receipt: &Receipt<'_>,
    report: &Capabilities<'_>,
) -> Result<String, Error> {
    envelope::wrap_json(receipt, report, &receipt.ceiling())
}

/// Returns `value` as an indented JSON document with a final line break and no receipt: the form
/// of the patch documents and of the vote report.
///
/// # Errors
///
/// Returns the serializer's error for a value it cannot write.
pub fn document_json<T: Serialize + ?Sized>(value: &T) -> Result<String, Error> {
    let mut text = json::to_text(value, JsonForm::Indented)?;
    text.push('\n');
    Ok(text)
}

/// Returns the verdicts document of a validate run, with the receipt's fields, bounded at the
/// standard ceiling with the chars4 counter, as the receipt of its run record.
#[must_use]
pub fn validate_jsonl(receipt: &Receipt<'_>, report: &ValidateReport) -> String {
    report.jsonl(&receipt.fields(&Ceiling::standard(&Chars4)))
}
