//! Complexity units, source line counts, and clone blocks with their keys, from one parse.

mod clones;
mod digest;
mod kinds;
mod sloc;
mod stream;
mod units;

use kernel::Key;
use pycompat::TokenizeError;
use syntax::{Language, Parsed, Point, QueryRunError};

use crate::kinds::Kinds;

/// The cc above which a unit counts as complex.
pub const DEFAULT_THRESHOLD: u32 = 10;
/// The fewest SLOC lines that a clone block holds.
pub const MIN_SLOC: usize = 3;
/// The fewest blocks that share a Type-2 key to make a clone group.
pub const MIN_INSTANCES: usize = 2;

// The metrics algorithm version, then the Python version whose tokenizer and Unicode tables the
// SLOC rule reproduces.
const STAMP: &str = "2/py3.13";

/// The units, clone blocks, and SLOC lines of one parsed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMetrics {
    /// The units by start byte.
    pub units: Vec<Unit>,
    /// The clone blocks by start line, the outer block first.
    pub blocks: Vec<Block>,
    /// The sorted numbers of the lines that hold code.
    pub sloc_lines: Vec<u32>,
}

/// One function or method with its qualified name, span, body key, cc, and SLOC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// The scope, owner, and name, joined with dots.
    pub qualname: String,
    /// The byte offset where the unit starts.
    pub start_byte: u32,
    /// The byte offset where the unit ends, exclusive.
    pub end_byte: u32,
    /// The 1-based line where the unit starts.
    pub start_line: u32,
    /// The digest of the unit's token stream without its name.
    pub body_key: Key,
    /// The cyclomatic complexity: 1 plus the decision points.
    pub cc: u32,
    /// The count of SLOC lines in the unit's line span.
    pub sloc: u32,
}

/// One function or control block that clone detection compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    /// The 1-based line where the block starts.
    pub start_line: u32,
    /// The 1-based line where the block ends.
    pub end_line: u32,
    /// The Type-2 key: identifiers and literals abstracted.
    pub key: Key,
    /// The exact key: every token as written.
    pub exact: Key,
    /// Whether the block is a function.
    pub function: bool,
    /// The count of identifiers, literals, and nodes that keep no child.
    pub tokens: u32,
}

/// The reason a parsed file has no metrics.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unmeasurable {
    /// The language has no metrics rules.
    #[error("{language:?} has no metrics rules")]
    Unsupported {
        /// The language of the parse.
        language: Language,
    },
    /// The tree holds an ERROR or MISSING node.
    #[error("the source does not parse at line {line}")]
    DoesNotParse {
        /// The 1-based line of the first ERROR or MISSING node.
        line: u32,
    },
    /// The Python tokenizer rejects the source.
    #[error("{0}")]
    Tokenizer(TokenizeError),
    /// The docstring query did not run to its end.
    #[error("the docstring query failed: {0}")]
    DocstringQuery(QueryRunError),
}

/// Measures the units, clone blocks, and SLOC lines of one parsed file.
///
/// # Errors
///
/// Returns [`Unmeasurable`] when the language has no rules, the tree holds a syntax error, the
/// tokenizer rejects a Python source, or the docstring query stops at its match limit.
///
/// # Panics
///
/// Panics when the constant docstring query does not compile.
pub fn measure(parsed: &Parsed) -> Result<FileMetrics, Unmeasurable> {
    let language = parsed.language();
    let spec = language
        .spec()
        .ok_or(Unmeasurable::Unsupported { language })?;
    if let Some(node) = parsed.first_error() {
        let line = line(parsed.start_point(node));
        return Err(Unmeasurable::DoesNotParse { line });
    }
    let kinds = Kinds::new(language);
    let sloc_lines = sloc::lines(parsed, spec, &kinds)?;
    Ok(FileMetrics {
        units: units::units(parsed, spec, &kinds, &sloc_lines),
        blocks: clones::blocks(parsed, spec, &kinds, &sloc_lines),
        sloc_lines,
    })
}

/// Returns the version stamp of stored metrics for `language`, or `None` when it has no rules.
#[must_use]
pub fn stamp(language: Language) -> Option<&'static str> {
    language.spec().map(|_| STAMP)
}

/// Returns the order that blocks are listed in: path, then start line, then end line.
#[must_use]
pub fn place<'a>(path: &'a str, block: &Block) -> (&'a str, u32, u32) {
    (path, block.start_line, block.end_line)
}

fn line(point: Point) -> u32 {
    point.row + 1
}

// The count of sorted `lines` from `start` to `end`, both inclusive.
fn lines_within(lines: &[u32], start: u32, end: u32) -> usize {
    let after = lines.partition_point(|&line| line <= end);
    after.saturating_sub(lines.partition_point(|&line| line < start))
}

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;
    use std::path::Path;

    pub(crate) fn cases<T: DeserializeOwned>(file: &str) -> T {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(format!("{file}.json"));
        let shown = path.display();
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{shown}: {error}"));
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{shown}: {error}"))
    }
}
