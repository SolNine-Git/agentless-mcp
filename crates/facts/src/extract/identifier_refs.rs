use std::collections::{HashMap, HashSet};

use syntax::{Extraction, KindSet, NodeId, RoleRule};

use super::python_scopes::{self, Roles};
use super::rust_uses::inline_depth;
use super::tree::{Field, Tree};
use crate::values::{Ref, Role};

const KEY_QUOTES: [char; 2] = ['"', '\''];
const RUST_UNNAMED: [&str; 2] = ["lifetime", "label"];
const RUST_PATHS: [&str; 2] = ["scoped_identifier", "scoped_type_identifier"];

// Every identifier occurrence in document order, with its role where the language has a role
// analysis and `reference` where it has none.
pub(super) fn refs(tree: &Tree, extraction: &Extraction) -> Vec<Ref> {
    let roles = (extraction.roles == RoleRule::Python).then(|| python_scopes::roles(tree));
    let paths = (extraction.roles == RoleRule::Rust).then(|| RustPaths::of(tree));
    let wanted = &extraction.identifiers;
    tree.preorder(tree.root())
        .filter(|node| wanted.contains(tree.parsed.kind(*node)))
        .filter(|node| {
            paths
                .as_ref()
                .is_none_or(|paths| !paths.unnamed.contains(node))
        })
        .filter_map(|node| {
            let name = identifier_name(tree, node, wanted);
            let mut found =
                (!name.is_empty()).then(|| occurrence(tree, node, name, roles.as_ref()))?;
            if let Some(qualifier) = paths.as_ref().and_then(|paths| paths.qualifiers.get(&node)) {
                qualifier.clone_into(&mut found.qualifier);
            }
            Some(found)
        })
        .collect()
}

// The identifiers inside Rust lifetimes and labels, which name no symbol, and the module path
// that qualifies the last segment of each `a::b::name` whose path is all modules.
#[derive(Default)]
struct RustPaths {
    unnamed: HashSet<NodeId>,
    qualifiers: HashMap<NodeId, String>,
}

impl RustPaths {
    fn of(tree: &Tree) -> RustPaths {
        let mut paths = RustPaths::default();
        for node in tree.preorder(tree.root()) {
            if tree.is_any(node, &RUST_UNNAMED) {
                paths.unnamed.extend(tree.preorder(node).skip(1));
            } else if tree.is_any(node, &RUST_PATHS) {
                paths.qualify(tree, node);
            }
        }
        paths
    }

    fn qualify(&mut self, tree: &Tree, node: NodeId) {
        let (Some(path), Some(name)) =
            (tree.field(node, Field::Path), tree.field(node, Field::Name))
        else {
            return;
        };
        let text: String = tree
            .text(path)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if module_like(&text) && !relative_inside_inline_module(tree, node, &text) {
            self.qualifiers.insert(name, text);
        }
    }
}

// The resolver walks a qualifier from the file, so a `self` or `super` path inside an inline
// module qualifies nothing and its name falls back.
fn relative_inside_inline_module(tree: &Tree, node: NodeId, path: &str) -> bool {
    let first = path.split("::").next().unwrap_or_default();
    matches!(first, "self" | "super") && inline_depth(tree, node) > 0
}

// A module path: each segment starts lower case, as `crate`, `self`, `super` and module names do.
fn module_like(path: &str) -> bool {
    !path.is_empty()
        && path.split("::").all(|segment| {
            segment
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_lowercase() || first == '_')
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
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
    KEY_QUOTES
        .iter()
        .find(|quote| text.len() >= 2 && text.starts_with(**quote) && text.ends_with(**quote))
        .map_or(text, |_| &text[1..text.len() - 1])
}
