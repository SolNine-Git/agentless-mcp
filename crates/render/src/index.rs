//! The index report's two forms: the summary line with at most ten error rows and ten warning
//! rows, and the whole report as a JSON document under its receipt.

use app::IndexReport;
use text::one_line;

use crate::envelope::wrap_json_whole;
use crate::{Error, Receipt};

// Each kind of row stops here and counts the rest, so a large failure stays one screen.
const FAILURE_LINES: usize = 10;

/// Returns the summary line of `report` and its first ten error and warning rows.
#[must_use]
pub fn index_text(report: &IndexReport) -> String {
    let errors = report.failures.iter();
    let errors = errors.map(|failure| (failure.path.as_str(), failure.reason.as_str()));
    let warnings = report.skipped_files.iter();
    let warnings = warnings.map(|skip| (skip.path.as_str(), skip.reason.as_str()));
    let mut lines = vec![report.summary_line()];
    lines.extend(rows("error", "errors", errors));
    lines.extend(rows("warning", "warnings", warnings));
    lines.join("\n") + "\n"
}

// The first rows of one kind, then a count of the rows past the cut. A file name is repository
// text, so a line break in it is escaped and cannot open a line of its own.
fn rows<'r>(
    kind: &str,
    noun: &str,
    entries: impl ExactSizeIterator<Item = (&'r str, &'r str)>,
) -> Vec<String> {
    let total = entries.len();
    let shown = entries.take(FAILURE_LINES);
    let mut lines: Vec<String> = shown
        .map(|(path, reason)| format!("  {kind}: {}: {}", one_line(path), one_line(reason)))
        .collect();
    if total > FAILURE_LINES {
        let more = total - FAILURE_LINES;
        lines.push(format!("  ... {more} more {noun} not listed"));
    }
    lines
}

/// Returns `report` as a JSON document that starts with its receipt.
///
/// # Errors
///
/// Returns the serializer's error for a value it cannot write.
pub fn index_json(receipt: &Receipt<'_>, report: &IndexReport) -> Result<String, Error> {
    wrap_json_whole(receipt, report, &receipt.ceiling())
}
