//! The patch commands' text forms: one line per checked file, the receipt with the failed edits,
//! and the blocks that did not parse.

use app::{ApplyResult, BlockError, CheckReport, FileCheck};

use crate::blocks::{blocks_or, detail_suffix};
use crate::receipt::{Receipt, receipt_lines};

/// Returns the check report as one line per checked file, or "no files were edited".
#[must_use]
pub fn check_text(report: &CheckReport) -> String {
    blocks_or(&report.files, check_line, "\n", "no files were edited")
}

fn check_line(check: &FileCheck) -> String {
    let Some(verdict) = &check.verdict else {
        return format!("{}: {}", check.path, check.error);
    };
    let status = if verdict.ok { "ok" } else { "BROKEN" };
    format!(
        "{}: {status} ({}, errors {} -> {}){}",
        check.path,
        verdict.language,
        verdict.old_errors,
        verdict.new_errors,
        detail_suffix(&verdict.detail)
    )
}

/// Returns the stderr text of a patch command: the receipt lines with `summary`, then one line
/// per edit that did not apply.
#[must_use]
pub fn patch_receipt(receipt: &Receipt<'_>, summary: &str, result: &ApplyResult) -> String {
    let mut lines = receipt_lines(receipt, summary);
    lines.extend(result.failures().map(|outcome| {
        let edit = &outcome.edit;
        format!(
            "  {}: {} block {}: {}",
            outcome.status.as_str(),
            edit.path,
            edit.index,
            outcome.reason
        )
    }));
    lines.join("\n") + "\n"
}

/// Returns the line that names one block that did not parse, and why.
#[must_use]
pub fn block_line(error: &BlockError) -> String {
    let path = error.path.as_deref().filter(|path| !path.is_empty());
    format!(
        "  block {} ({}): {}",
        error.index,
        path.unwrap_or("no path"),
        error.reason
    )
}

/// Returns the stderr text of a parse with blocks that did not parse: their count, then a line
/// for each.
#[must_use]
pub fn parse_errors(errors: &[BlockError]) -> String {
    let mut lines = vec![format!(
        "agentless-mcp: {} blocks did not parse",
        errors.len()
    )];
    lines.extend(errors.iter().map(block_line));
    lines.join("\n") + "\n"
}
