//! The require and source calls: `require("mod")` in Lua and Ruby, and `source file` and
//! `. file` in shell scripts.

use syntax::NodeId;

use super::tree::{Field, Tree};
use crate::spelling::strip_quotes;
use crate::values::Import;

const CALLS: [&str; 3] = ["call", "call_expression", "function_call"];
const STRINGS: [&str; 2] = ["string", "string_literal"];
const SOURCE_COMMANDS: [&str; 2] = ["source", "."];

pub(super) fn requires(tree: &Tree) -> Vec<Import> {
    tree.each_where(|node| tree.is_any(node, &CALLS), |node| require(tree, node))
}

// A call of `require` imports the module its first string argument names.
fn require(tree: &Tree, node: NodeId) -> Option<Import> {
    let function = tree
        .field(node, Field::Function)
        .or_else(|| tree.children(node).next())?;
    if tree.text(function) != "require" {
        return None;
    }
    let arguments = tree.field(node, Field::Arguments)?;
    let path = strip_quotes(tree.text(tree.first_child_of(arguments, &STRINGS)?));
    (!path.is_empty()).then(|| Import::at(tree, node, path, path.starts_with('.')))
}

pub(super) fn sources(tree: &Tree) -> Vec<Import> {
    tree.each_of_kind("command", |node| source(tree, node))
}

// `source path` and `. path`: the second word, with its quote characters off both ends.
fn source(tree: &Tree, node: NodeId) -> Option<Import> {
    let mut words = tree.children(node);
    let command = words.next()?;
    if !SOURCE_COMMANDS.contains(&tree.text(command)) {
        return None;
    }
    let path = tree.text(words.next()?).trim_matches(['\'', '"']);
    Some(Import::at(tree, node, path, !path.starts_with('/')))
}
