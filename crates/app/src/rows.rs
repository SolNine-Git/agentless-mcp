//! The coverage rows: the not-checked findings that say which part of a change no check read.

use checks::{Finding, PairFinding, Severity};
use prompts::{Template, Text};

/// The check name of a coverage row.
pub const COVERAGE: &str = "coverage";

// One not-checked row with no line. An empty path is the repository.
pub(crate) fn coverage(message: String, path: &str, evidence: String) -> PairFinding {
    PairFinding {
        finding: Finding {
            check: COVERAGE,
            severity: Severity::NotChecked,
            message,
            path: path.to_owned(),
            line: 0,
            evidence,
        },
        detail: None,
    }
}

// A side that was not read, with the reason the read gave.
pub(crate) fn side_row(
    template: &Template<(Text, Text)>,
    path: &str,
    reason: String,
) -> PairFinding {
    coverage(template.render((path, &reason)), path, reason)
}

// A path that resolves outside the repository. The refusal names no raw argument.
pub(crate) fn refused_row(refusal: &io::Refusal) -> PairFinding {
    coverage(
        format!("not checked: {refusal}"),
        "",
        "path refused".to_owned(),
    )
}

// A coverage row in the error position of a step. A row is large, so it is boxed there.
pub(crate) type Gap = Box<PairFinding>;

// The text of one side: decoded, or the row that names the read failure.
pub(crate) fn side_text(
    template: &Template<(Text, Text)>,
    path: &str,
    read: Result<Vec<u8>, String>,
) -> Result<String, Gap> {
    read.and_then(|bytes| io::decode_strict(bytes).map_err(|error| error.to_string()))
        .map_err(|reason| Box::new(side_row(template, path, reason)))
}

// A bounded read of a working-tree file, with the skip text as the error.
pub(crate) fn bounded_read(path: &std::path::Path) -> Result<Vec<u8>, String> {
    io::read_bounded(path).map_err(|skipped| skipped.to_string())
}
