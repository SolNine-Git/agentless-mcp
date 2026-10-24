//! The card that every view shows for one symbol, the rationale nodes linked to a symbol, and the
//! cards' text form, which the body budget measures.

use facts::{StableId, Symbol, rationale_handle};
use syntax::Language;
use text::one_line;

use crate::rowtext::{ROW_INDENT, locator};

/// One symbol in one place: its id, span, kind, language, signature, and the body lines a view
/// carries, with how many of them the view shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolCard {
    /// The stable id.
    pub stable_id: String,
    /// The path below the repository root.
    pub path: String,
    /// The first line.
    pub start_line: u32,
    /// The last line.
    pub end_line: u32,
    /// The kind's name, such as `method`.
    pub kind: &'static str,
    /// The language's name, such as `python`.
    pub language: &'static str,
    /// The one-line signature.
    pub signature: String,
    /// The owner that qualifies the name, or empty.
    pub parent_class: String,
    /// The body lines, joined by newlines, or empty.
    pub body: String,
    /// How many body lines the card shows.
    pub body_shown: usize,
    /// How many body lines the symbol has.
    pub body_total: usize,
}

/// One rationale comment and the symbol node it is linked to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RationaleNode {
    /// The rationale handle.
    pub stable_id: String,
    /// The stable id of the symbol that holds the comment.
    pub parent_id: String,
    /// The line of the marker or the citation.
    pub line: u32,
    /// The marker's kind, such as `why`.
    pub kind: &'static str,
    /// The comment text.
    pub text: String,
    /// The citations the comment names.
    pub citations: Vec<String>,
}

/// Returns the card of `symbol`, which the file at `path` declares in `language`, with `body` as
/// its whole body.
#[must_use]
pub fn symbol_card(language: Language, path: &str, symbol: &Symbol, body: String) -> SymbolCard {
    let lines = if body.is_empty() {
        0
    } else {
        body.split('\n').count()
    };
    SymbolCard {
        stable_id: StableId::of(language, path, symbol).to_string(),
        path: path.to_owned(),
        start_line: symbol.line,
        end_line: symbol.end_line,
        kind: symbol.kind.as_str(),
        language: language.name(),
        signature: symbol.signature.clone(),
        parent_class: symbol.parent.clone(),
        body,
        body_shown: lines,
        body_total: lines,
    }
}

/// Returns the rationale nodes of `symbol`, which the file at `path` declares in `language`.
#[must_use]
pub fn rationale_nodes(language: Language, path: &str, symbol: &Symbol) -> Vec<RationaleNode> {
    let parent_id = StableId::of(language, path, symbol).to_string();
    symbol
        .rationales
        .iter()
        .map(|rationale| RationaleNode {
            stable_id: rationale_handle(language, path, symbol, rationale),
            parent_id: parent_id.clone(),
            line: rationale.line,
            kind: rationale.kind.as_str(),
            text: rationale.text.clone(),
            citations: rationale.citations.clone(),
        })
        .collect()
}

/// Returns the text of `cards`: each card's locator, kind and body or signature, a blank line
/// between cards, or "no matching symbols" when there is no card.
#[must_use]
pub fn cards_text(cards: &[SymbolCard]) -> String {
    if cards.is_empty() {
        return "no matching symbols\n".to_owned();
    }
    let blocks: Vec<String> = cards.iter().map(card_text).collect();
    blocks.join("\n\n") + "\n"
}

// A method's owner is its class. A nested function's owner can be a function, so it reads "in".
fn card_text(card: &SymbolCard) -> String {
    let holder = if card.kind == "method" {
        "in class"
    } else {
        "in"
    };
    let owner = if card.parent_class.is_empty() {
        String::new()
    } else {
        format!(" {holder} {}", one_line(&card.parent_class))
    };
    let span = if card.start_line == card.end_line {
        card.start_line.to_string()
    } else {
        format!("{}-{}", card.start_line, card.end_line)
    };
    let kind = one_line(card.kind);
    let language = one_line(card.language);
    let mut lines = vec![
        locator(&card.stable_id, "", Some(&span)),
        format!("{ROW_INDENT}{kind}{owner} ({language})"),
    ];
    if card.body.is_empty() {
        lines.push(format!("{ROW_INDENT}{}", one_line(&card.signature)));
    } else {
        let body = card.body.split('\n');
        lines.extend(body.map(|line| format!("{ROW_INDENT}{}", one_line(line))));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests;
