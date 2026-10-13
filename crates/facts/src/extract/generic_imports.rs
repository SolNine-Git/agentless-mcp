//! The generic imports: one statement per import node anywhere in the tree, with the names it
//! binds by the language's binding shape.

use syntax::{Declarations, ImportBinding, NodeId};

use super::tree::Tree;
use crate::spelling::strip_quotes;
use crate::values::Import;

// The nodes that make a last-segment import bind no single name, or several.
const WILDCARDS: [&str; 3] = ["asterisk", "wildcard_import", "namespace_wildcard"];
const SELECTORS: &str = "namespace_selectors";
const ALIASES: &str = "import_alias";

pub(super) fn imports(tree: &Tree, declarations: &Declarations) -> Vec<Import> {
    tree.each_where(
        |node| declarations.imports.contains(tree.parsed.kind(node)),
        |node| import(tree, declarations, node),
    )
}

fn import(tree: &Tree, declarations: &Declarations, node: NodeId) -> Option<Import> {
    let path = import_path(tree, declarations, node);
    if path.is_empty() {
        return None;
    }
    let (names, local_names) = match declarations.import_binding {
        ImportBinding::Specifiers => {
            let bound = specifier_names(tree, node);
            (bound.clone(), bound)
        }
        ImportBinding::LastSegment => last_segment(tree, node, &path),
        ImportBinding::ModuleObject => (Vec::new(), Vec::new()),
    };
    Some(Import::at(tree, node, &path, path.starts_with('.')).naming(names, local_names))
}

// The path field's text, else the path kind among the children joined with dots, else among
// the children of the first child that holds one.
fn import_path(tree: &Tree, declarations: &Declarations, node: NodeId) -> String {
    let field = declarations.import_path_field;
    if let Some(path) = field.and_then(|field| tree.parsed.child_by_field(node, field)) {
        return strip_quotes(tree.text(path)).to_owned();
    }
    let kind = &declarations.import_path_kind;
    let segments = |parent: NodeId| -> Vec<NodeId> {
        tree.children(parent)
            .filter(|child| kind.contains(tree.parsed.kind(*child)))
            .collect()
    };
    let direct = segments(node);
    let found = if direct.is_empty() {
        tree.children(node)
            .map(segments)
            .find(|nested| !nested.is_empty())
            .unwrap_or_default()
    } else {
        direct
    };
    let parts: Vec<&str> = found
        .iter()
        .map(|segment| strip_quotes(tree.text(*segment)))
        .collect();
    parts.join(".")
}

// `import { X, Y as Z }` and `import D from`: the local names the statement lists, in order.
fn specifier_names(tree: &Tree, node: NodeId) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for child in tree.preorder(node) {
        let identifiers = tree
            .children(child)
            .filter(|part| tree.is(*part, "identifier"));
        let listed: Vec<NodeId> = match tree.kind(child) {
            "import_specifier" => identifiers.last().into_iter().collect(),
            "import_clause" => identifiers.collect(),
            _ => Vec::new(),
        };
        for name in listed.into_iter().map(|part| tree.text(part)) {
            if !names.iter().any(|known| known == name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

// `import com.acme.Money;`: the path ends on the name it binds. A wildcard binds nothing, a
// selector list binds each name it lists, and an alias replaces the local name.
fn last_segment(tree: &Tree, node: NodeId, path: &str) -> (Vec<String>, Vec<String>) {
    let subtree: Vec<NodeId> = tree.preorder(node).collect();
    if subtree.iter().any(|child| tree.is_any(*child, &WILDCARDS)) {
        return (Vec::new(), Vec::new());
    }
    let mut selected: Vec<String> = Vec::new();
    let listed = subtree
        .iter()
        .filter(|child| tree.is(**child, SELECTORS))
        .flat_map(|child| tree.named_children(*child));
    for name in listed.map(|listed| tree.text(listed)) {
        if !selected.iter().any(|known| known == name) {
            selected.push(name.to_owned());
        }
    }
    if !selected.is_empty() {
        return (selected.clone(), selected);
    }
    let dotted = path.replace('\\', ".");
    let member = dotted.rsplit('.').next().unwrap_or_default();
    if member.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let alias = subtree
        .iter()
        .filter(|child| tree.is(**child, ALIASES))
        .find_map(|child| tree.named_children(*child).next())
        .map_or(member, |named| tree.text(named));
    let alias = if alias.is_empty() { member } else { alias };
    (vec![member.to_owned()], vec![alias.to_owned()])
}
