use syntax::NodeId;

use super::tree::{Field, Tree};
use crate::values::Import;

const RELATIVE_ROOTS: [&str; 3] = ["self", "super", "crate"];

// One import per `use` declaration anywhere in the tree, `mod` bodies included.
pub(super) fn imports(tree: &Tree) -> Vec<Import> {
    tree.each_of_kind("use_declaration", |node| import(tree, node))
}

// The module the declaration's paths share, and each use-tree leaf as a member path below it
// with its local name: `use a::{b, c::{d, e as f}}` gives module `a` and `b`, `c::d`, `c::e`.
fn import(tree: &Tree, node: NodeId) -> Option<Import> {
    let argument = tree.field(node, Field::Argument)?;
    let mut leaves: Vec<(String, String)> = Vec::new();
    let module = match tree.kind(argument) {
        "scoped_identifier" => split_last(&spaceless(tree.text(argument)), None, &mut leaves),
        "use_as_clause" => {
            let alias = field_text(tree, argument, Field::Alias);
            let path = field_text(tree, argument, Field::Path);
            split_last(&path, Some(alias), &mut leaves)
        }
        "scoped_use_list" => {
            if let Some(list) = tree.field(argument, Field::List) {
                collect(tree, list, "", &mut leaves);
            }
            field_text(tree, argument, Field::Path)
        }
        "use_list" => {
            collect(tree, argument, "", &mut leaves);
            String::new()
        }
        "use_wildcard" => wildcard_path(tree, argument),
        _ => spaceless(tree.text(argument)),
    };
    if module.is_empty() && leaves.is_empty() {
        return None;
    }
    let first = module.split("::").next().unwrap_or_default();
    let (names, locals): (Vec<String>, Vec<String>) = leaves.into_iter().unzip();
    let mut import =
        Import::at(tree, node, &module, RELATIVE_ROOTS.contains(&first)).naming(names, locals);
    import.binds_all = tree.is(argument, "use_wildcard");
    import.inline_depth = inline_depth(tree, node);
    Some(import)
}

// `a::b::c` gives the module `a::b` and the leaf `c`, bound under `alias` when one is given.
fn split_last(path: &str, alias: Option<String>, leaves: &mut Vec<(String, String)>) -> String {
    let (module, last) = path.rsplit_once("::").unwrap_or(("", path));
    leaves.push((last.to_owned(), alias.unwrap_or_else(|| last.to_owned())));
    module.to_owned()
}

// Each leaf below `node` as (member path below the declaration's module, local name).
fn collect(tree: &Tree, node: NodeId, prefix: &str, leaves: &mut Vec<(String, String)>) {
    match tree.kind(node) {
        "use_list" => {
            for child in tree.named_children(node) {
                collect(tree, child, prefix, leaves);
            }
        }
        "scoped_use_list" => {
            let inner = joined(prefix, &field_text(tree, node, Field::Path));
            if let Some(list) = tree.field(node, Field::List) {
                collect(tree, list, &inner, leaves);
            }
        }
        "line_comment" | "block_comment" => {}
        _ => leaves.push(leaf(tree, node, prefix)),
    }
}

// A glob binds under its own path, an alias under the alias, any other leaf under its last
// segment.
fn leaf(tree: &Tree, node: NodeId, prefix: &str) -> (String, String) {
    match tree.kind(node) {
        "use_wildcard" => {
            let path = wildcard_path(tree, node);
            let glob = if path.is_empty() {
                "*".to_owned()
            } else {
                format!("{path}::*")
            };
            let member = joined(prefix, &glob);
            (member.clone(), member)
        }
        "use_as_clause" => {
            let member = joined(prefix, &field_text(tree, node, Field::Path));
            let alias = tree
                .field(node, Field::Alias)
                .map_or("", |alias| tree.text(alias));
            (member, alias.to_owned())
        }
        _ => {
            let member = joined(prefix, &spaceless(tree.text(node)));
            let local = member.rsplit("::").next().unwrap_or_default().to_owned();
            (member, local)
        }
    }
}

fn joined(prefix: &str, path: &str) -> String {
    match (prefix.is_empty(), path.is_empty()) {
        (true, _) => path.to_owned(),
        (false, true) => prefix.to_owned(),
        (false, false) => format!("{prefix}::{path}"),
    }
}

// The path before `::*`, or empty for a bare `*`.
fn wildcard_path(tree: &Tree, wildcard: NodeId) -> String {
    let path = tree.named_children(wildcard).next();
    path.map_or(String::new(), |path| spaceless(tree.text(path)))
}

// How many inline `mod name { ... }` bodies enclose the node.
pub(super) fn inline_depth(tree: &Tree, node: NodeId) -> u32 {
    let ancestors = std::iter::successors(tree.parsed.parent(node), |up| tree.parsed.parent(*up));
    let modules = ancestors.filter(|up| tree.is(*up, "mod_item")).count();
    u32::try_from(modules).expect("a file nests fewer than 2^32 modules")
}

fn field_text(tree: &Tree, node: NodeId, field: Field) -> String {
    tree.field(node, field)
        .map_or(String::new(), |found| spaceless(tree.text(found)))
}

fn spaceless(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}
