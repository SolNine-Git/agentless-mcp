use syntax::{Extraction, KindSet, NodeId, RoleRule};

use super::python_scopes::{self, Roles};
use super::tree::Tree;
use crate::values::{Ref, Role};

const QUOTES: [char; 2] = ['"', '\''];

// Every identifier occurrence in document order, with its role where the language has a role
// analysis and `reference` where it has none.
pub(super) fn refs(tree: &Tree, extraction: &Extraction) -> Vec<Ref> {
    let roles = (extraction.roles == RoleRule::Python).then(|| python_scopes::roles(tree));
    let wanted = &extraction.identifiers;
    tree.preorder(tree.root())
        .filter(|node| wanted.contains(tree.parsed.kind(*node)))
        .filter_map(|node| {
            let name = identifier_name(tree, node, wanted);
            (!name.is_empty()).then(|| occurrence(tree, node, name, roles.as_ref()))
        })
        .collect()
}

fn occurrence(tree: &Tree, node: NodeId, name: &str, roles: Option<&Roles>) -> Ref {
    let (role, qualifier) = roles.map_or((Role::Reference, ""), |roles| {
        (roles.role(tree, node, name), roles.qualifier(node))
    });
    Ref {
        name: name.to_owned(),
        line: tree.line(node),
        role,
        qualifier: qualifier.to_owned(),
    }
}

// The name a node spells, or empty when it holds another identifier and so is a path or a
// container; one layer of matching quotes comes off a delimited form.
fn identifier_name<'a>(tree: &Tree<'a>, node: NodeId, wanted: &KindSet) -> &'a str {
    if tree
        .named_children(node)
        .any(|child| wanted.contains(tree.parsed.kind(child)))
    {
        return "";
    }
    let text = tree.text(node);
    QUOTES
        .iter()
        .find(|quote| text.len() >= 2 && text.starts_with(**quote) && text.ends_with(**quote))
        .map_or(text, |_| &text[1..text.len() - 1])
}
