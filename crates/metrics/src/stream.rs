use std::borrow::Cow;

use kernel::Key;
use syntax::{NodeId, Parsed, Spec};

use crate::digest::Joined;
use crate::word::is_word;

// Two control characters mark an indent block's bounds. A string literal that holds one raw can
// collide with them, which at worst makes the keys of two files equal.
const BLOCK_OPEN: &str = "\x01";
const BLOCK_CLOSE: &str = "\x02";

const MARKERS: [&str; 4] = ["#", "//", "/*", "--"];
const DIRECTIVE_PREFIXES: [&str; 15] = [
    "!",
    "go:",
    "-*-",
    "type:",
    "pragma:",
    "@ts-",
    "eslint-",
    "ruff:",
    "pylint:",
    "mypy:",
    "flake8:",
    "coverage:",
    "fmt:",
    "isort:",
    "prettier-",
];
// Each of these ends in a word character, so it needs a word boundary after it.
const DIRECTIVE_WORDS: [&str; 3] = ["+build", "noqa", "yapf"];

enum Frame {
    Node(NodeId),
    Close,
}

enum Visit<'a> {
    Skip,
    Emit(Cow<'a, str>),
    Enter { block: bool },
}

pub(crate) fn body_key(parsed: &Parsed, spec: &Spec, unit: NodeId, omit: NodeId) -> Key {
    let mut joined = Joined::new(" ");
    for token in tokens(parsed, spec, unit, omit) {
        joined.piece(&[&token]);
    }
    joined.key()
}

// The leaf texts in document order, with directive comments kept and the subtree `omit` left out.
fn tokens<'a>(parsed: &'a Parsed, spec: &Spec, root: NodeId, omit: NodeId) -> Vec<Cow<'a, str>> {
    let mut found = Vec::new();
    let mut stack = vec![Frame::Node(root)];
    while let Some(frame) = stack.pop() {
        let Frame::Node(node) = frame else {
            found.push(Cow::Borrowed(BLOCK_CLOSE));
            continue;
        };
        if node == omit {
            continue;
        }
        match visit(parsed, spec, node) {
            Visit::Skip => {}
            Visit::Emit(text) => found.push(text),
            Visit::Enter { block } => {
                if block {
                    found.push(Cow::Borrowed(BLOCK_OPEN));
                    stack.push(Frame::Close);
                }
                let first = stack.len();
                stack.extend(parsed.children(node).map(Frame::Node));
                stack[first..].reverse();
            }
        }
    }
    found
}

fn visit<'a>(parsed: &'a Parsed, spec: &Spec, node: NodeId) -> Visit<'a> {
    let kind = parsed.kind(node);
    if spec.comments.contains(kind) {
        return directive_text(parsed.text(node))
            .map_or(Visit::Skip, |text| Visit::Emit(text.into()));
    }
    if parsed.child_count(node) == 0 {
        let text = parsed.text(node);
        return if text.is_empty() {
            Visit::Skip
        } else {
            Visit::Emit(text.into())
        };
    }
    Visit::Enter {
        block: spec.clones.indent_blocks.contains(kind),
    }
}

// A directive comment keeps its whitespace-collapsed text. A prose comment drops out of the stream.
fn directive_text(comment: &str) -> Option<String> {
    let text = text::split_whitespace(comment)
        .collect::<Vec<_>>()
        .join(" ");
    is_directive(without_marker(&text)).then_some(text)
}

// The collapsed text has no leading space and single spaces inside, so at most one space follows
// the marker.
fn without_marker(text: &str) -> &str {
    MARKERS
        .iter()
        .find_map(|marker| text.strip_prefix(marker))
        .map_or(text, |rest| rest.strip_prefix(' ').unwrap_or(rest))
}

fn is_directive(body: &str) -> bool {
    DIRECTIVE_PREFIXES
        .iter()
        .any(|prefix| body.starts_with(prefix))
        || DIRECTIVE_WORDS.iter().any(|word| {
            body.strip_prefix(word)
                .is_some_and(|rest| !rest.chars().next().is_some_and(is_word))
        })
}

#[cfg(test)]
mod tests {
    use text::testing::document;

    use super::directive_text;

    #[test]
    fn directive_text_cases() {
        let found: Vec<(String, String)> = document(env!("CARGO_MANIFEST_DIR"), "directives");
        assert!(!found.is_empty());
        for (comment, expected) in found {
            let text = directive_text(&comment).unwrap_or_default();
            assert_eq!(text, expected, "{comment:?}");
        }
    }
}
