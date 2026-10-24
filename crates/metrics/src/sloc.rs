use std::collections::BTreeSet;

use syntax::{NodeId, Parsed, Point, SlocRule, Spec};

use crate::kinds::Kinds;
use crate::tokenize::python_sloc;
use crate::{Unmeasurable, line};

pub(crate) fn lines(parsed: &Parsed, spec: &Spec, kinds: &Kinds) -> Result<Vec<u32>, Unmeasurable> {
    match spec.sloc {
        SlocRule::Leaves => Ok(leaf_lines(parsed, spec)),
        SlocRule::PythonTokens => python_lines(parsed, spec, kinds),
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
fn python_lines(parsed: &Parsed, spec: &Spec, kinds: &Kinds) -> Result<Vec<u32>, Unmeasurable> {
    let source = parsed.source();
    let tokens = python_sloc(source).map_err(Unmeasurable::Tokenizer)?;
    let text_lines = text::splitlines(source, false);
    let docstrings: BTreeSet<u32> = statement_strings(parsed, spec, kinds)
        .filter(|&node| is_docstring(parsed, kinds, node, &text_lines))
        .flat_map(|node| line(parsed.start_point(node))..=line(parsed.end_point(node)))
        .collect();
    Ok(tokens.difference(&docstrings).copied().collect())
}

// The statement-level strings: a named `string` child of a statement parent.
fn statement_strings<'a>(
    parsed: &'a Parsed,
    spec: &'a Spec,
    kinds: &'a Kinds,
) -> impl Iterator<Item = NodeId> + 'a {
    let parents = spec.statement_parents.as_ref();
    parsed.descendants(parsed.root()).filter(move |&node| {
        parsed.is_named(node)
            && kinds.string.contains(parsed.kind(node))
            && parsed
                .parent(node)
                .is_some_and(|parent| parents.is_some_and(|set| set.contains(parsed.kind(parent))))
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
    use syntax::{Language, SlocRule};

    // A docstring is a string child of a block or of the module, so the scan reads those parents.
    #[test]
    fn the_token_rule_reads_the_block_and_the_module_as_statement_parents() {
        let rules = Language::ALL
            .into_iter()
            .filter_map(|language| Some((language, language.spec()?)))
            .filter(|(_, spec)| spec.sloc == SlocRule::PythonTokens);
        let mut count = 0;
        for (language, spec) in rules {
            let expected = language.kinds(&["block", "module"]);
            assert_eq!(
                spec.statement_parents.as_ref(),
                Some(&expected),
                "{language:?}"
            );
            count += 1;
        }
        assert_eq!(count, 1);
    }
}
