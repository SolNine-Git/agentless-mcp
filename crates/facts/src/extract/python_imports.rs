use syntax::NodeId;

use super::tree::{Field, Tree};
use crate::values::Import;

pub(super) fn imports(tree: &Tree) -> Vec<Import> {
    tree.preorder(tree.root())
        .flat_map(|node| match tree.kind(node) {
            "import_statement" => bare(tree, node),
            "import_from_statement" => vec![from(tree, node)],
            _ => Vec::new(),
        })
        .collect()
}

// `import a.b` and `import a.b as ab`: one statement per module.
fn bare(tree: &Tree, node: NodeId) -> Vec<Import> {
    tree.children(node)
        .filter_map(|child| match tree.kind(child) {
            "dotted_name" => Some(Import::at(tree, node, tree.text(child), false)),
            "aliased_import" => {
                let name = tree.field(child, Field::Name)?;
                let mut import = Import::at(tree, node, tree.text(name), false);
                if let Some(alias) = tree.field(child, Field::Alias) {
                    tree.text(alias).clone_into(&mut import.alias);
                }
                Some(import)
            }
            _ => None,
        })
        .collect()
}

// `from x import a, b as c` and `from . import *`. The local names stay even where they equal
// the members.
fn from(tree: &Tree, node: NodeId) -> Import {
    let module = tree.field(node, Field::ModuleName);
    let (name, level) = from_module(tree, node, module);
    let (names, local_names) = members(tree, node, module);
    Import {
        binds_all: names.iter().any(|name| name == "*"),
        names,
        local_names,
        relative_level: level,
        ..Import::at(tree, node, &name, level > 0)
    }
}

fn from_module(tree: &Tree, node: NodeId, module: Option<NodeId>) -> (String, u32) {
    match module {
        Some(module) if tree.is(module, "relative_import") => relative(tree, module),
        Some(module) => (tree.text(module).to_owned(), 0),
        None => (String::new(), dots(tree, node)),
    }
}

// `from ..pkg import x`: the dots give the level and the dotted name the module.
fn relative(tree: &Tree, module: NodeId) -> (String, u32) {
    let mut name = String::new();
    let mut level = 0;
    for part in tree.children(module) {
        match tree.kind(part) {
            "import_prefix" => level = code_points(tree.text(part)),
            "dotted_name" => tree.text(part).clone_into(&mut name),
            _ => {}
        }
    }
    (name, level)
}

// With no module name, the level is the length of the first child that is all dots.
fn dots(tree: &Tree, node: NodeId) -> u32 {
    tree.children(node)
        .filter(|child| !tree.is_any(*child, &["import", "from"]))
        .map(|child| tree.text(child))
        .find(|text| !text.is_empty() && text.chars().all(|c| c == '.'))
        .map_or(0, code_points)
}

// The members a statement names and the local names they bind, by position.
fn members(tree: &Tree, node: NodeId, module: Option<NodeId>) -> (Vec<String>, Vec<String>) {
    let mut names = Vec::new();
    let mut local_names = Vec::new();
    for child in tree.children(node) {
        let pair = match tree.kind(child) {
            "dotted_name" if Some(child) != module => {
                let text = tree.text(child);
                Some((text, text))
            }
            "aliased_import" => aliased(tree, child),
            "wildcard_import" => Some(("*", "*")),
            _ => None,
        };
        if let Some((name, local)) = pair {
            names.push(name.to_owned());
            local_names.push(local.to_owned());
        }
    }
    (names, local_names)
}

fn aliased<'a>(tree: &Tree<'a>, child: NodeId) -> Option<(&'a str, &'a str)> {
    let name = tree.text(tree.field(child, Field::Name)?);
    let local = tree
        .field(child, Field::Alias)
        .map_or(name, |alias| tree.text(alias));
    Some((name, local))
}

fn code_points(text: &str) -> u32 {
    u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
}
