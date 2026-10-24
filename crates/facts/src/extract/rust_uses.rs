use syntax::NodeId;

use super::tree::{Field, Tree};
use crate::values::Import;

const RELATIVE_ROOTS: [&str; 3] = ["self", "super", "crate"];

// One import per `use` declaration anywhere in the tree, `mod` bodies included.
pub(super) fn imports(tree: &Tree) -> Vec<Import> {
    tree.each_of_kind("use_declaration", |node| import(tree, node))
}

fn import(tree: &Tree, node: NodeId) -> Option<Import> {
    let argument = tree.field(node, Field::Argument)?;
    let target = target(tree, argument);
    if target.module.is_empty() {
        return None;
    }
    let first = target.module.split("::").next().unwrap_or_default();
    let mut import = Import::at(tree, node, target.module, RELATIVE_ROOTS.contains(&first))
        .naming(target.names, target.local_names);
    target.alias.clone_into(&mut import.alias);
    Some(import)
}

struct Target<'a> {
    module: &'a str,
    names: Vec<String>,
    local_names: Vec<String>,
    alias: &'a str,
}

// `use a::b as c` keeps the path and the alias; `use a::{b, c as d}` keeps the path and lists.
fn target<'a>(tree: &Tree<'a>, argument: NodeId) -> Target<'a> {
    let text = |field| {
        tree.field(argument, field)
            .map_or("", |node| tree.text(node))
    };
    match tree.kind(argument) {
        "use_as_clause" => Target {
            module: text(Field::Path),
            names: Vec::new(),
            local_names: Vec::new(),
            alias: text(Field::Alias),
        },
        "scoped_use_list" => {
            let entries: Vec<(String, String)> = tree
                .field(argument, Field::List)
                .map(|list| {
                    tree.named_children(list)
                        .map(|entry| entry_names(tree, entry))
                        .collect()
                })
                .unwrap_or_default();
            let (names, local_names) = entries.into_iter().unzip();
            Target {
                module: text(Field::Path),
                names,
                local_names,
                alias: "",
            }
        }
        _ => Target {
            module: tree.text(argument),
            names: Vec::new(),
            local_names: Vec::new(),
            alias: "",
        },
    }
}

fn entry_names(tree: &Tree, entry: NodeId) -> (String, String) {
    if !tree.is(entry, "use_as_clause") {
        let text = tree.text(entry).to_owned();
        return (text.clone(), text);
    }
    let text = |field| tree.field(entry, field).map_or("", |node| tree.text(node));
    (text(Field::Path).to_owned(), text(Field::Alias).to_owned())
}
