use std::collections::BTreeSet;
use std::sync::OnceLock;

use syntax::{Language, NodeId, Parsed, Point, Query, SlocRule, Spec};

use crate::kinds::Kinds;
use crate::tokenize::python_sloc;
use crate::{Unmeasurable, line};

// The statement-level strings: a `string` child of a block or of the module.
const STATEMENT_STRINGS: &str =
    "(block (string) @statement_string) (module (string) @statement_string)";

pub(crate) fn lines(parsed: &Parsed, spec: &Spec, kinds: &Kinds) -> Result<Vec<u32>, Unmeasurable> {
    match spec.sloc {
        SlocRule::Leaves => Ok(leaf_lines(parsed, spec)),
        SlocRule::PythonTokens => python_lines(parsed, kinds),
    }
}

// The start line of each leaf with text, outside comments.
fn leaf_lines(parsed: &Parsed, spec: &Spec) -> Vec<u32> {
    let mut lines = BTreeSet::new();
    let mut stack = vec![parsed.root()];
    while let Some(node) = stack.pop() {
        if spec.comments.contains(parsed.kind(node)) {
            continue;
        }
        if parsed.child_count(node) > 0 {
            stack.extend(parsed.children(node));
        } else if !text::strip(parsed.text(node)).is_empty() {
            lines.insert(line(parsed.start_point(node)));
        }
    }
    lines.into_iter().collect()
}

// The tokenizer's code lines, without the lines of a docstring that owns its lines.
fn python_lines(parsed: &Parsed, kinds: &Kinds) -> Result<Vec<u32>, Unmeasurable> {
    let source = parsed.source();
    let tokens = python_sloc(source).map_err(Unmeasurable::Tokenizer)?;
    let captures = parsed
        .captures(docstring_query())
        .map_err(Unmeasurable::DocstringQuery)?;
    let text_lines = text::splitlines(source, false);
    let docstrings: BTreeSet<u32> = captures
        .iter()
        .filter(|capture| is_docstring(parsed, kinds, capture.node, &text_lines))
        .flat_map(|capture| {
            line(parsed.start_point(capture.node))..=line(parsed.end_point(capture.node))
        })
        .collect();
    Ok(tokens.difference(&docstrings).copied().collect())
}

fn docstring_query() -> &'static Query {
    static QUERY: OnceLock<Query> = OnceLock::new();
    QUERY.get_or_init(|| {
        Query::new(Language::Python, STATEMENT_STRINGS).expect("the docstring query compiles")
    })
}

fn is_docstring(parsed: &Parsed, kinds: &Kinds, node: NodeId, text_lines: &[&str]) -> bool {
    kinds.is_plain_string(parsed, node)
        && owns_its_lines(parsed.start_point(node), parsed.end_point(node), text_lines)
}

// Byte columns index the code points of the split lines, so non-ASCII text before a column shifts
// the test. The lines split at more breaks than the tree's rows do. Parity keeps both.
fn owns_its_lines(start: Point, end: Point, text_lines: &[&str]) -> bool {
    let line_at = |row: u32| text_lines.get(row as usize).copied().unwrap_or("");
    let (before, _) = split_at_char(line_at(start.row), start.column);
    let (_, after) = split_at_char(line_at(end.row), end.column);
    text::strip(before).is_empty() && text::strip(after).is_empty()
}

fn split_at_char(text: &str, count: u32) -> (&str, &str) {
    let at = text
        .char_indices()
        .nth(count as usize)
        .map_or(text.len(), |(index, _)| index);
    text.split_at(at)
}

#[cfg(test)]
mod tests {
    use super::docstring_query;

    #[test]
    fn the_docstring_query_compiles() {
        assert_eq!(docstring_query().pattern_count(), 2);
    }
}
