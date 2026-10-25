//! The nodes that a grammar puts before a definition node and that belong to the definition:
//! decorators, attributes and overload signatures. A grammar that nests them inside the
//! definition node needs no rule here.

use syntax::{Language, NodeId};

use super::tree::{Field, Tree};

struct Rule {
    // The parent kind that holds the definition and the decorators before it.
    wrapper: &'static str,
    // The sibling kinds before the definition that belong to it.
    leading: &'static [&'static str],
    // The signature kinds that declare an overload of the next definition of the same name.
    overloads: &'static [&'static str],
}

const PYTHON: Rule = Rule {
    wrapper: "decorated_definition",
    leading: &[],
    overloads: &[],
};
const RUST: Rule = Rule {
    wrapper: "",
    leading: &["attribute_item"],
    overloads: &[],
};
const JAVASCRIPT: Rule = Rule {
    wrapper: "export_statement",
    leading: &["decorator"],
    overloads: &[],
};
const TYPESCRIPT: Rule = Rule {
    wrapper: "export_statement",
    leading: &["decorator"],
    overloads: &["function_signature", "method_signature"],
};

fn rule(language: Language) -> Option<&'static Rule> {
    match language {
        Language::Python => Some(&PYTHON),
        Language::Rust => Some(&RUST),
        Language::Javascript => Some(&JAVASCRIPT),
        Language::Typescript | Language::Tsx => Some(&TYPESCRIPT),
        _ => None,
    }
}

// The line of the first decorator, attribute or overload signature of `definition`, or the line
// where `definition` starts when it has none.
pub(super) fn first_line(tree: &Tree, definition: NodeId) -> u32 {
    let Some(rule) = rule(tree.parsed.language()) else {
        return tree.line(definition);
    };
    let head = tree
        .parsed
        .parent(definition)
        .filter(|parent| tree.is(*parent, rule.wrapper))
        .unwrap_or(definition);
    let first = leading(tree, head, definition, rule).unwrap_or(head);
    tree.line(first)
}

// The first sibling of the attached run that ends right before `head`. A comment or an anonymous
// token inside the run neither starts nor ends it.
fn leading(tree: &Tree, head: NodeId, definition: NodeId, rule: &Rule) -> Option<NodeId> {
    if rule.leading.is_empty() && rule.overloads.is_empty() {
        return None;
    }
    let comments = &tree.parsed.language().extraction().comments;
    let mut first = None;
    let mut sibling = tree.parsed.prev_sibling(head);
    while let Some(node) = sibling {
        if attached(tree, node, definition, rule) {
            first = Some(node);
        } else if tree.parsed.is_named(node) && !comments.contains(tree.parsed.kind(node)) {
            break;
        }
        sibling = tree.parsed.prev_sibling(node);
    }
    first
}

fn attached(tree: &Tree, sibling: NodeId, definition: NodeId, rule: &Rule) -> bool {
    if tree.is_any(sibling, rule.leading) {
        return true;
    }
    let signature = if tree.is(sibling, rule.wrapper) {
        tree.first_child_of(sibling, rule.overloads)
    } else {
        Some(sibling)
    };
    signature.is_some_and(|signature| {
        tree.is_any(signature, rule.overloads) && same_name(tree, signature, definition)
    })
}

fn same_name(tree: &Tree, one: NodeId, other: NodeId) -> bool {
    let name = |node| tree.field(node, Field::Name).map(|name| tree.text(name));
    name(one).is_some_and(|spelled| name(other) == Some(spelled))
}
