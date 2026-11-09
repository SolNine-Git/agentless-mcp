//! The search pattern: the regular expression in the ripgrep dialect, compiled line by line in
//! linear time, and the fault of a pattern that does not compile.

use regex::{Regex, RegexBuilder};

// regex prints the pattern under this indent and a caret line under the fault.
const PATTERN_INDENT: &str = "    ";
const REASON_PREFIX: &str = "error: ";

/// A pattern that does not compile, with the pattern that matches its text literally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternError {
    /// The pattern as the caller wrote it.
    pub pattern: String,
    /// The pattern with each special character escaped, which matches the text itself.
    pub escaped: String,
    /// Why the pattern does not compile.
    pub fault: PatternFault,
}

/// Why a pattern does not compile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatternFault {
    /// The pattern does not parse.
    Syntax {
        /// The parser's reason, such as `unclosed group`.
        reason: String,
        /// The 1-based character column of the fault, when the parser marks one.
        column: Option<usize>,
    },
    /// The compiled pattern passes the size limit, in bytes.
    TooBig {
        /// The limit.
        limit: usize,
    },
    /// Any other refusal, as the regex crate words it.
    Other(String),
}

// The pattern matches inside one line: `^` and `$` are the line's ends and `.` stops at none.
pub(super) fn compiled(pattern: &str, ignore_case: bool) -> Result<Regex, PatternError> {
    let built = RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build();
    built.map_err(|error| PatternError {
        pattern: pattern.to_owned(),
        escaped: regex::escape(pattern),
        fault: fault(&error),
    })
}

fn fault(error: &regex::Error) -> PatternFault {
    match error {
        regex::Error::Syntax(message) => syntax(message),
        regex::Error::CompiledTooBig(limit) => PatternFault::TooBig { limit: *limit },
        other => PatternFault::Other(other.to_string()),
    }
}

// The parser's message holds the pattern line, a caret line under the fault, and the reason. A
// message of another shape keeps its whole text as the reason, with no column.
fn syntax(message: &str) -> PatternFault {
    let lines: Vec<&str> = message.lines().collect();
    let reason = lines
        .iter()
        .find_map(|line| line.strip_prefix(REASON_PREFIX));
    let Some(reason) = reason else {
        return PatternFault::Syntax {
            reason: message.to_owned(),
            column: None,
        };
    };
    let column = lines
        .iter()
        .find_map(|line| caret(line))
        .map(|offset| offset + 1);
    PatternFault::Syntax {
        reason: reason.to_owned(),
        column,
    }
}

// The offset of the first caret on a line that holds only the indent, spaces and carets.
fn caret(line: &str) -> Option<usize> {
    let marks = line.strip_prefix(PATTERN_INDENT)?;
    let only_marks = marks.contains('^') && marks.bytes().all(|byte| matches!(byte, b' ' | b'^'));
    only_marks.then(|| marks.find('^')).flatten()
}
