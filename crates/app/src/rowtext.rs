//! The row grammar that every text answer shares, and that the budgets of the map, the bodies and
//! the history measure: one home for each fragment a reader scans for.

use facts::StableId;
use prompts::messages::STABLE_IDS_PATTERN;
use text::one_line;

/// The indent of a row under a grouped header.
pub const ROW_INDENT: &str = "  ";

/// What a row names in place of a symbol for a line that no symbol holds.
pub const MODULE_LEVEL: &str = "(module level)";

// What the id pattern of a grouped view stands in for the symbol's own name.
const QUALIFIED_NAME_PLACEHOLDER: &str = "<QualifiedName>";

/// Returns the bracketed stable id that a row is read for, linked to `parent` when it is not
/// empty, and followed by ` @line` when `line` is given.
#[must_use]
pub fn locator(stable_id: &str, parent: &str, line: Option<&str>) -> String {
    let named = if parent.is_empty() {
        one_line(stable_id).into_owned()
    } else {
        format!("{} -> {}", one_line(stable_id), one_line(parent))
    };
    match line {
        Some(line) => format!("[{named}] @{line}"),
        None => format!("[{named}]"),
    }
}

/// Returns the id pattern that a grouped view prints once under the header of `path`, or an
/// empty text when `id_prefix` is empty.
#[must_use]
pub fn stable_ids_line(id_prefix: &str, path: &str) -> String {
    if id_prefix.is_empty() {
        return String::new();
    }
    let pattern = StableId::new(id_prefix, path, QUALIFIED_NAME_PLACEHOLDER).to_string();
    let line = STABLE_IDS_PATTERN.render(&one_line(&pattern));
    format!("{ROW_INDENT}{line}")
}

/// Returns the `path:line` fragment of a row that names no symbol.
#[must_use]
pub fn file_site(path: &str, line: u32) -> String {
    format!("{}:{line}", one_line(path))
}

/// Returns the line that announces a listing's cut: how many more of `noun` the answer leaves
/// out, and the limit that cut it when `limit` is not zero.
#[must_use]
pub fn omitted_line(omitted: usize, noun: &str, limit: usize) -> String {
    let bound = if limit > 0 {
        format!(" (limit {limit})")
    } else {
        String::new()
    };
    format!("... {omitted} more {noun} not listed{bound}")
}
