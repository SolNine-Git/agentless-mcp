//! The action names of each unit: the names it calls and the fields it reads.

use std::collections::BTreeSet;

use syntax::{Parsed, Spec};

use crate::Unit;

// One token of the file as the rule reads it: a name, or any other token by its text.
#[derive(Clone, Copy)]
enum Token<'a> {
    Name(&'a str),
    Other(&'a str),
}

// Each unit's names, in the order of `units`.
pub(crate) fn actions(parsed: &Parsed, spec: &Spec, units: &[Unit]) -> Vec<Vec<String>> {
    let tokens = tokens(parsed, spec);
    units
        .iter()
        .map(|unit| unit_actions(&tokens, unit))
        .collect()
}

// The file's tokens in document order, each with its start byte. A literal is one token and a
// comment is none, so no name inside either one counts.
fn tokens<'a>(parsed: &'a Parsed, spec: &Spec) -> Vec<(u32, Token<'a>)> {
    let mut tokens = Vec::new();
    let mut skipped_to = 0;
    for node in parsed.descendants(parsed.root()) {
        let start = parsed.start_byte(node);
        if start < skipped_to {
            continue;
        }
        let kind = parsed.kind(node);
        let literal = spec.clones.literal_class(kind).is_some();
        if literal || spec.comments.contains(kind) {
            skipped_to = parsed.end_byte(node);
            if literal {
                tokens.push((start, Token::Other(parsed.text(node))));
            }
        } else if parsed.is_leaf(node) {
            let text = parsed.text(node);
            let token = if spec.clones.identifiers.contains(kind) {
                Token::Name(text)
            } else {
                Token::Other(text)
            };
            tokens.push((start, token));
        }
    }
    tokens
}

// A name before `(` is called, and a name after `.` and not before `(` is a field read. The
// unit's own name never counts, and neither does its last token.
fn unit_actions(tokens: &[(u32, Token<'_>)], unit: &Unit) -> Vec<String> {
    let first = tokens.partition_point(|(start, _)| *start < unit.start_byte);
    let after = tokens.partition_point(|(start, _)| *start < unit.end_byte);
    let span: Vec<Token<'_>> = tokens[first..after]
        .iter()
        .map(|(_, token)| *token)
        .collect();
    let own = unit.qualname.rsplit('.').next().unwrap_or_default();
    let mut names = BTreeSet::new();
    for (at, window) in span.windows(2).enumerate() {
        let Token::Name(name) = window[0] else {
            continue;
        };
        let called = matches!(window[1], Token::Other("("));
        let read = !called && at > 0 && matches!(span[at - 1], Token::Other("."));
        if name != own && (called || read) {
            names.insert(name);
        }
    }
    names.into_iter().map(str::to_owned).collect()
}

#[cfg(test)]
mod tests;
