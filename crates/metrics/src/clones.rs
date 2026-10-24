use std::cmp::Reverse;
use std::collections::HashMap;

use kernel::Key;
use syntax::{NodeId, Parsed, Spec};

use crate::digest::Joined;
use crate::kinds::Kinds;
use crate::{Block, MIN_SLOC, line, lines_within};

enum Item {
    Node(NodeId),
    Token(&'static str),
}

struct CloneKey {
    type2: Key,
    exact: Key,
    tokens: u32,
}

// The pieces of both keys, written as the walk meets them.
struct Streams<'a> {
    parsed: &'a Parsed,
    spec: &'a Spec,
    kinds: &'a Kinds,
    names: HashMap<&'a str, String>,
    type2: Joined,
    exact: Joined,
    tokens: u32,
}

// The candidate blocks by start line, outer block first. A stable sort keeps the walk order of
// blocks with the same span.
pub(crate) fn blocks(
    parsed: &Parsed,
    spec: &Spec,
    kinds: &Kinds,
    sloc_lines: &[u32],
) -> Vec<Block> {
    let rules = &spec.clones;
    let mut found = Vec::new();
    let mut stack = vec![parsed.root()];
    while let Some(node) = stack.pop() {
        stack.extend(parsed.children(node));
        let kind = parsed.kind(node);
        let function = rules.functions.contains(kind);
        if !(function || rules.blocks.contains(kind)) || rules.skips(parsed, node) {
            continue;
        }
        let start_line = line(parsed.start_point(node));
        let end_line = line(parsed.end_point(node));
        if lines_within(sloc_lines, start_line, end_line) < MIN_SLOC {
            continue;
        }
        let key = clone_key(parsed, spec, kinds, node);
        found.push(Block {
            start_line,
            end_line,
            key: key.type2,
            exact: key.exact,
            function,
            tokens: key.tokens,
        });
    }
    found.sort_by_key(|block| (block.start_line, Reverse(block.end_line)));
    found
}

// The Type-2 key numbers identifiers by first use and names literals by class. The exact key
// keeps every token as written.
fn clone_key(parsed: &Parsed, spec: &Spec, kinds: &Kinds, root: NodeId) -> CloneKey {
    let mut streams = Streams {
        parsed,
        spec,
        kinds,
        names: HashMap::new(),
        type2: Joined::new(""),
        exact: Joined::new("\x00"),
        tokens: 0,
    };
    let mut stack = vec![Item::Node(root)];
    while let Some(item) = stack.pop() {
        match item {
            Item::Token(token) => streams.piece(&[token], &[token]),
            Item::Node(node) => streams.node(node, &mut stack),
        }
    }
    CloneKey {
        type2: streams.type2.key(),
        exact: streams.exact.key(),
        tokens: streams.tokens,
    }
}

impl Streams<'_> {
    fn piece(&mut self, type2: &[&str], exact: &[&str]) {
        self.type2.piece(type2);
        self.exact.piece(exact);
    }

    fn node(&mut self, node: NodeId, stack: &mut Vec<Item>) {
        let parsed = self.parsed;
        let kind = parsed.kind(node);
        let name = parsed.kind_name(node);
        if self.spec.clones.identifiers.contains(kind) {
            let text = parsed.text(node);
            let next = self.names.len() + 1;
            let label = self
                .names
                .entry(text)
                .or_insert_with(|| format!("$VAR{next}"));
            self.type2.piece(&[label]);
            self.exact.piece(&[name, "=", text]);
            self.tokens += 1;
        } else if let Some(class) = self.spec.clones.literal_class(kind) {
            self.piece(&["$", class.as_str()], &[name, "=", parsed.text(node)]);
            self.tokens += 1;
        } else {
            let kept = kept_children(parsed, self.spec, self.kinds, node);
            if kept.is_empty() {
                self.piece(&[name], &[name]);
                self.tokens += 1;
            } else {
                self.piece(&[name, "("], &[name, "("]);
                push_opened(&kept, stack);
            }
        }
    }
}

// The stack pops the children first to last, with a comma between two and a close after the last.
fn push_opened(kept: &[NodeId], stack: &mut Vec<Item>) {
    stack.push(Item::Token(")"));
    for (index, &child) in kept.iter().rev().enumerate() {
        stack.push(Item::Node(child));
        if index + 1 < kept.len() {
            stack.push(Item::Token(","));
        }
    }
}

fn kept_children(parsed: &Parsed, spec: &Spec, kinds: &Kinds, node: NodeId) -> Vec<NodeId> {
    parsed
        .children(node)
        .filter(|&child| {
            !spec.comments.contains(parsed.kind(child))
                && !is_docstring_statement(parsed, spec, kinds, child, node)
        })
        .collect()
}

// A plain-string statement drops out with no ownership test, and so does a tuple that holds only
// one plain string.
fn is_docstring_statement(
    parsed: &Parsed,
    spec: &Spec,
    kinds: &Kinds,
    child: NodeId,
    parent: NodeId,
) -> bool {
    let Some(parents) = &spec.statement_parents else {
        return false;
    };
    if !parents.contains(parsed.kind(parent)) {
        return false;
    }
    if kinds.tuple_expression.contains(parsed.kind(child)) {
        let mut named = parsed.named_children(child);
        return parsed.named_child_count(child) == 1
            && named
                .next()
                .is_some_and(|only| kinds.is_plain_string(parsed, only));
    }
    kinds.is_plain_string(parsed, child)
}
