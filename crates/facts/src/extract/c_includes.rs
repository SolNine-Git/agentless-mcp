use syntax::NodeId;

use super::tree::Tree;
use crate::spelling::strip_quotes;
use crate::values::Import;

const PATHS: [&str; 2] = ["string_literal", "system_lib_string"];

// One import per `#include` anywhere in the tree, include guards included. An include is a
// textual paste, so it binds every name the header declares.
pub(super) fn imports(tree: &Tree) -> Vec<Import> {
    tree.each_of_kind("preproc_include", |node| directive(tree, node))
}

fn directive(tree: &Tree, node: NodeId) -> Option<Import> {
    let path = tree.first_child_of(node, &PATHS)?;
    let text = tree.text(path);
    let module = strip_quotes(text.trim_matches(['<', '>']));
    let mut import = Import::at(tree, node, module, !text.starts_with('<'));
    import.binds_all = true;
    Some(import)
}
