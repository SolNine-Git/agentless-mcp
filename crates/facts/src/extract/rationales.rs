use syntax::Extraction;

use super::tree::Tree;
use crate::pattern::{citations, marker};
use crate::spelling::{lstrip, rstrip, truncate};
use crate::values::{Rationale, RationaleKind, Symbol};

const MAX_RATIONALE_CHARS: usize = 240;
const COMMENT_PREFIXES: [&str; 5] = ["<!--", "//", "/*", "#", "*"];
const COMMENT_SUFFIXES: [&str; 2] = ["-->", "*/"];

// Each marker line and each line with a citation, of every comment node, in document order.
pub(super) fn rationales(tree: &Tree, extraction: &Extraction) -> Vec<Rationale> {
    let comments = tree
        .preorder(tree.root())
        .filter(|node| extraction.comments.contains(tree.parsed.kind(*node)));
    comments
        .flat_map(|node| {
            let first = tree.line(node);
            // Rows count line feeds only, so a comment splits on them alone.
            tree.text(node)
                .split('\n')
                .zip(first..)
                .filter_map(|(raw, line)| rationale(comment_text(raw), line))
        })
        .collect()
}

// Hang each rationale off the symbol that opens last before it and still contains it; the first
// such symbol wins a tie.
pub(super) fn attached(mut symbols: Vec<Symbol>, rationales: Vec<Rationale>) -> Vec<Symbol> {
    for mut rationale in rationales {
        let Some(owner) = owner(&symbols, rationale.line) else {
            continue;
        };
        let siblings = &mut symbols[owner].rationales;
        let same_line = siblings
            .iter()
            .filter(|sibling| sibling.line == rationale.line);
        rationale.duplicate_index = u32::try_from(same_line.count()).unwrap_or(u32::MAX);
        siblings.push(rationale);
    }
    symbols
}

fn owner(symbols: &[Symbol], line: u32) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (position, symbol) in symbols.iter().enumerate() {
        let contains = symbol.line <= line && line <= symbol.end_line;
        if contains && best.is_none_or(|best| symbols[best].line < symbol.line) {
            best = Some(position);
        }
    }
    best
}

fn rationale(line_text: &str, line: u32) -> Option<Rationale> {
    let found = marker(line_text);
    let cited: Vec<String> = citations(line_text)
        .into_iter()
        .map(|citation| text::strip(citation).to_owned())
        .collect();
    if found.is_none() && cited.is_empty() {
        return None;
    }
    let (kind, text) = match found {
        Some((kind, rest)) => (kind, text::strip(rest)),
        None => (RationaleKind::Citation, line_text),
    };
    let text = if text.is_empty() {
        cited.join(", ")
    } else {
        text.to_owned()
    };
    Some(Rationale {
        kind,
        text: truncate(&text, MAX_RATIONALE_CHARS),
        line,
        citations: cited,
        duplicate_index: 0,
    })
}

// One comment line without the delimiters that open or close it.
fn comment_text(raw: &str) -> &str {
    let text = text::strip(raw);
    let text = COMMENT_PREFIXES
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .map_or(text, lstrip);
    COMMENT_SUFFIXES
        .iter()
        .find_map(|suffix| text.strip_suffix(suffix))
        .map_or(text, rstrip)
}
