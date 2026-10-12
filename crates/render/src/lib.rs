//! The text and JSON forms of the answers: the receipt envelope, the lint report, and the
//! capabilities report. Every function returns the bytes to print and does no I/O.

mod capabilities;
mod count;
mod envelope;
mod lint;
mod receipt;
#[cfg(test)]
mod tests;
mod textsafe;

use app::{Capabilities, LintReport};

pub use receipt::Receipt;
pub use textsafe::one_line;

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
    envelope::wrap(receipt, &lint::render_lint(report), count::ceiling())
}

/// Returns the lint report as a JSON document that starts with its receipt.
///
/// # Errors
///
/// Returns the serializer's error for a value it cannot write.
pub fn lint_json(receipt: &Receipt<'_>, report: &LintReport) -> Result<String, Error> {
    envelope::wrap_json(receipt, report, count::ceiling())
}

/// Returns the capabilities report as text under its receipt.
#[must_use]
pub fn capabilities_text(receipt: &Receipt<'_>, report: &Capabilities<'_>) -> String {
    let body = capabilities::render_capabilities(report);
    envelope::wrap(receipt, &body, count::ceiling())
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
    envelope::wrap_json(receipt, report, count::ceiling())
}
