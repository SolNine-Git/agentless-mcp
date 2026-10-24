//! Whether an edited file parses no worse than it did before the edit.

use std::sync::Arc;

use serde::Serialize;
use syntax::{Language, Parsed, Unparsable};

/// The parse-error counts of a file before and after an edit. `ok` means the edit added no
/// ERROR or MISSING node, and `checked` means a parser ran at all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SyntaxVerdict {
    pub language: &'static str,
    pub old_errors: usize,
    pub new_errors: usize,
    pub ok: bool,
    pub checked: bool,
    /// The 1-based line of the new text's first error, in document order.
    pub first_error_line: Option<u64>,
    pub detail: String,
}

/// Compares the parse errors of `new` with those of `old`. Without a language the verdict is
/// `ok` and not `checked`.
///
/// # Errors
///
/// Returns the parser's refusal of either text.
pub fn syntax_delta(
    old: &str,
    new: &str,
    language: Option<Language>,
) -> Result<SyntaxVerdict, Unparsable> {
    let Some(language) = language else {
        return Ok(SyntaxVerdict {
            language: "unknown",
            old_errors: 0,
            new_errors: 0,
            ok: true,
            checked: false,
            first_error_line: None,
            detail: "no grammar for this file: syntax was not checked".to_owned(),
        });
    };
    let new_tree = syntax::parse(language, Arc::from(new))?;
    let old_errors = error_count(&syntax::parse(language, Arc::from(old))?);
    let new_errors = error_count(&new_tree);
    let first_error_line = new_tree
        .first_error()
        .map(|node| u64::from(new_tree.start_point(node).row) + 1);
    Ok(SyntaxVerdict {
        language: language.name(),
        old_errors,
        new_errors,
        ok: new_errors <= old_errors,
        checked: true,
        first_error_line,
        detail: detail(old_errors, new_errors, first_error_line),
    })
}

// The ERROR and MISSING nodes, without the fragments under an ERROR node: they are consequences
// of that one failure.
fn error_count(parsed: &Parsed) -> usize {
    let mut count = 0;
    let mut stack = vec![parsed.root()];
    while let Some(node) = stack.pop() {
        if parsed.is_missing(node) || parsed.is_error(node) {
            count += 1;
        } else if parsed.has_error(node) {
            stack.extend(parsed.children(node));
        }
    }
    count
}

fn detail(old_errors: usize, new_errors: usize, first_error_line: Option<u64>) -> String {
    if new_errors > old_errors {
        let added = format!(
            "edit introduced {} new parse errors",
            new_errors - old_errors
        );
        return match first_error_line {
            Some(line) => format!("{added}, first at line {line}"),
            None => added,
        };
    }
    if old_errors > 0 {
        return format!("file already had {old_errors} parse errors before the edit");
    }
    String::new()
}
