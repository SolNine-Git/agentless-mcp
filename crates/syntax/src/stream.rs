use std::borrow::Cow;

use text::is_word;

use crate::{KindSet, Language, NodeId, Parsed};

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

// The kinds that the stream reads in one grammar: the comments, and the blocks whose bounds the
// grammar hides. Only a measured language has indent blocks.
pub(crate) struct StreamKinds {
    pub(crate) comments: &'static KindSet,
    pub(crate) indent_blocks: Option<&'static KindSet>,
}

impl StreamKinds {
    pub(crate) fn of(language: Language) -> StreamKinds {
        StreamKinds {
            comments: &language.extraction().comments,
            indent_blocks: language.spec().map(|spec| &spec.clones.indent_blocks),
        }
    }
}

/// Returns the leaf texts under `root` in document order, with directive comments kept, a marker
/// around each indent block, and the subtree `omit` left out.
#[must_use]
pub fn tokens(parsed: &Parsed, root: NodeId, omit: Option<NodeId>) -> Vec<Cow<'_, str>> {
    let kinds = StreamKinds::of(parsed.language());
    let mut found = Vec::new();
    let mut stack = vec![Frame::Node(root)];
    while let Some(frame) = stack.pop() {
        let Frame::Node(node) = frame else {
            found.push(Cow::Borrowed(BLOCK_CLOSE));
            continue;
        };
        if omit == Some(node) {
            continue;
        }
        match visit(parsed, &kinds, node) {
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

fn visit<'a>(parsed: &'a Parsed, kinds: &StreamKinds, node: NodeId) -> Visit<'a> {
    let kind = parsed.kind(node);
    if kinds.comments.contains(kind) {
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
        block: kinds
            .indent_blocks
            .is_some_and(|blocks| blocks.contains(kind)),
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

    // The metrics generator writes the directive cases beside the metrics fixtures.
    #[test]
    fn directive_text_cases() {
        let metrics = concat!(env!("CARGO_MANIFEST_DIR"), "/../metrics");
        let found: Vec<(String, String)> = document(metrics, "directives");
        assert!(!found.is_empty());
        for (comment, expected) in found {
            let text = directive_text(&comment).unwrap_or_default();
            assert_eq!(text, expected, "{comment:?}");
        }
    }
}
