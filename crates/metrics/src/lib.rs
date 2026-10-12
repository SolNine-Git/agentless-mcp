//! Complexity units, source line counts, and clone blocks with their keys, from one parse.

mod clones;
mod digest;
mod kinds;
mod sloc;
mod stream;
mod sum;
mod tokenize;
mod units;
mod word;

pub use sum::compensated_sum;
pub use tokenize::{TokenizeError, TokenizeErrorKind};

use kernel::Key;
use syntax::{Language, NodeId, Parsed, Point, Spec};

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
    /// Whether a function kind, not a control-block kind, made the block.
    pub function: bool,
    /// The count of identifiers, literals, and nodes that keep no child.
    pub tokens: u32,
}

/// The reason a parsed file has no metrics.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Unmeasurable {
    /// The language has no metrics rules.
    #[error("{language:?} has no metrics rules")]
    Unsupported { language: Language },
    /// The tree holds an ERROR or MISSING node, visible or hidden.
    #[error("the source does not parse at line {line}")]
    DoesNotParse {
        /// The 1-based line of the first visible ERROR or MISSING node, or else of the innermost
        /// node that holds the error.
        line: u32,
    },
    /// The Python tokenizer rejects the source.
    #[error("{0}")]
    Tokenizer(TokenizeError),
}

/// Measures the units, clone blocks, and SLOC lines of one parsed file.
///
/// # Errors
///
/// Returns [`Unmeasurable`] when the language has no rules, the tree holds a syntax error, or the
/// tokenizer rejects a Python source.
pub fn measure(parsed: &Parsed) -> Result<FileMetrics, Unmeasurable> {
    let (spec, kinds, sloc_lines) = checked(parsed)?;
    Ok(FileMetrics {
        units: units::units(parsed, spec, &kinds, &sloc_lines),
        blocks: clones::blocks(parsed, spec, &kinds, &sloc_lines),
        sloc_lines,
    })
}

/// A parsed file that measures, with its SLOC lines: the source of its units and its blocks.
pub struct Measurable {
    parsed: Parsed,
    spec: &'static Spec,
    kinds: Kinds,
    sloc_lines: Vec<u32>,
}

impl Measurable {
    /// Checks that `parsed` measures, and finds its SLOC lines.
    ///
    /// # Errors
    ///
    /// Returns [`Unmeasurable`] when the language has no rules, the tree holds a syntax error, or
    /// the tokenizer rejects a Python source.
    pub fn new(parsed: Parsed) -> Result<Measurable, Unmeasurable> {
        let (spec, kinds, sloc_lines) = checked(&parsed)?;
        Ok(Measurable {
            parsed,
            spec,
            kinds,
            sloc_lines,
        })
    }

    /// Returns the parse.
    #[must_use]
    pub fn parsed(&self) -> &Parsed {
        &self.parsed
    }

    /// Returns the units by start byte.
    #[must_use]
    pub fn units(&self) -> Vec<Unit> {
        units::units(&self.parsed, self.spec, &self.kinds, &self.sloc_lines)
    }

    /// Returns the clone blocks by start line, the outer block first.
    #[must_use]
    pub fn blocks(&self) -> Vec<Block> {
        clones::blocks(&self.parsed, self.spec, &self.kinds, &self.sloc_lines)
    }
}

// The rules and the SLOC lines of a parsed file that measures.
fn checked(parsed: &Parsed) -> Result<(&'static Spec, Kinds, Vec<u32>), Unmeasurable> {
    let language = parsed.language();
    let spec = language
        .spec()
        .ok_or(Unmeasurable::Unsupported { language })?;
    if let Some(node) = error_node(parsed) {
        let line = line(parsed.start_point(node));
        return Err(Unmeasurable::DoesNotParse { line });
    }
    let kinds = Kinds::new(language);
    let sloc_lines = sloc::lines(parsed, spec, &kinds)?;
    Ok((spec, kinds, sloc_lines))
}

// The guard keys on the root's error flag. A hidden MISSING token sets it with no visible ERROR or
// MISSING node, and then the innermost node that holds the error names the line.
fn error_node(parsed: &Parsed) -> Option<NodeId> {
    if !parsed.has_error(parsed.root()) {
        return None;
    }
    parsed.first_error().or_else(|| parsed.innermost_error())
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
    use kernel::Key;
    use syntax::{Language, parse};

    use super::{Block, Measurable, Unmeasurable, measure, place};

    // Recorded deviation: the reference crashes on a tree whose only error is a hidden MISSING
    // token, so these lines are the innermost erroneous node's, not a generated case.
    #[test]
    fn a_tree_with_only_a_hidden_error_does_not_parse_where_the_reference_crashes() {
        let cases = [
            (Language::C, "#include <stdio.h>", 1),
            (Language::Cpp, "#pragma once", 1),
            (Language::C, "int f(void) { return 0; }\n#define X 1", 2),
            (Language::Go, "package p\n\nf", 1),
        ];
        for (language, source, line) in cases {
            let parsed = parse(language, source.into()).expect("a tree");
            assert!(
                parsed.first_error().is_none(),
                "{source:?} has a visible error"
            );
            let refused = Some(Unmeasurable::DoesNotParse { line });
            assert_eq!(measure(&parsed).err(), refused, "{source:?}");
            assert_eq!(Measurable::new(parsed).err(), refused, "{source:?}");
        }
    }

    // Recorded deviation: the reference prints a SyntaxWarning line on stderr for each backslash
    // before a brace in an f-string, and this build has no warning channel.
    #[test]
    fn an_fstring_backslash_before_a_brace_measures_with_no_error() {
        let parsed = parse(Language::Python, "x = f\"\\{y}\"\n".into()).expect("a tree");
        let measured = measure(&parsed).expect("it measures");
        assert_eq!(measured.sloc_lines, [1]);
    }

    #[test]
    fn place_orders_by_path_then_start_line_then_end_line() {
        let block = |start_line, end_line| Block {
            start_line,
            end_line,
            key: Key::new([0; 32]),
            exact: Key::new([0; 32]),
            function: false,
            tokens: 0,
        };
        let (wide, narrow, later) = (block(2, 9), block(2, 5), block(3, 4));
        let mut placed = vec![
            place("b.py", &narrow),
            place("a.py", &later),
            place("a.py", &wide),
            place("a.py", &narrow),
        ];
        placed.sort_unstable();
        assert_eq!(
            placed,
            [
                ("a.py", 2, 5),
                ("a.py", 2, 9),
                ("a.py", 3, 4),
                ("b.py", 2, 5)
            ]
        );
    }
}
