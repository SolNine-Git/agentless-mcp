//! The Rust items: functions, structs, enums, traits, the methods of impl blocks, constants and
//! type aliases, in modules at any depth, and the names of the `mod` items.

use syntax::NodeId;

use super::declares_async;
use super::tree::{Field, Tree};
use crate::spelling::truncate;
use crate::values::{Symbol, SymbolKind};

const ITEMS: [&str; 7] = [
    "function_item",
    "struct_item",
    "enum_item",
    "trait_item",
    "impl_item",
    "const_item",
    "type_item",
];
const ADTS: [(&str, SymbolKind, &str); 3] = [
    ("struct_item", SymbolKind::Class, "struct"),
    ("enum_item", SymbolKind::Enum, "enum"),
    ("trait_item", SymbolKind::Protocol, "trait"),
];
const MAX_VALUE_CHARS: usize = 80;
const MAX_TYPE_ALIAS_CHARS: usize = 120;
const UNKNOWN: &str = "<unknown>";
// The items whose `cfg(test)` makes every function inside them test code.
const TEST_SCOPES: [&str; 2] = ["mod_item", "impl_item"];

pub(super) fn symbols(tree: &Tree) -> Vec<Symbol> {
    tree.each_declaration(&ITEMS, |node| item(tree, node))
}

pub(super) fn modules(tree: &Tree) -> Vec<String> {
    tree.each_of_kind("mod_item", |node| {
        tree.field(node, Field::Name)
            .map(|name| tree.text(name).to_owned())
    })
}

fn item(tree: &Tree, node: NodeId) -> Vec<Symbol> {
    match tree.kind(node) {
        "function_item" => vec![function(tree, node, "")],
        "impl_item" => methods(tree, node),
        "const_item" => constant(tree, node).into_iter().collect(),
        "type_item" => type_alias(tree, node).into_iter().collect(),
        _ => adt(tree, node).into_iter().collect(),
    }
}

// Public when the item carries any visibility modifier, `pub(crate)` included.
fn is_public(tree: &Tree, node: NodeId) -> bool {
    tree.children(node)
        .any(|child| tree.is(child, "visibility_modifier"))
}

fn function(tree: &Tree, node: NodeId, parent: &str) -> Symbol {
    let name_node = tree.field(node, Field::Name);
    let name = name_node.map_or(UNKNOWN, |name| tree.text(name));
    let parameters = tree
        .field(node, Field::Parameters)
        .map_or("()", |parameters| tree.text(parameters));
    let returns = tree
        .field(node, Field::ReturnType)
        .map_or(String::new(), |returns| {
            format!(" -> {}", tree.text(returns))
        });
    let prefix = if declares_async(tree, node) {
        "async fn"
    } else {
        "fn"
    };
    let kind = if parent.is_empty() {
        SymbolKind::Function
    } else {
        SymbolKind::Method
    };
    Symbol::at(tree, node, name, kind)
        .named_by(tree, name_node)
        .signed(&format!("{prefix} {name}{parameters}{returns}"))
        .owned_by(parent)
        .public(is_public(tree, node))
        .in_tests(is_test(tree, node))
}

// A test attribute on the function, or `cfg(test)` on it or on a module or impl block around it.
fn is_test(tree: &Tree, function: NodeId) -> bool {
    if attributes(tree, function).any(|attribute| is_test_attribute(tree, attribute)) {
        return true;
    }
    let scopes = std::iter::successors(Some(function), |node| tree.parsed.parent(*node))
        .filter(|node| *node == function || tree.is_any(*node, &TEST_SCOPES));
    scopes
        .flat_map(|scope| attributes(tree, scope))
        .any(|attribute| is_cfg_test(tree, attribute))
}

// The `attribute` node of each attribute item right before `item`, the nearest first. A comment
// between them does not end the run.
fn attributes<'t>(tree: &'t Tree, item: NodeId) -> impl Iterator<Item = NodeId> + 't {
    let comments = &tree.parsed.language().extraction().comments;
    std::iter::successors(tree.parsed.prev_sibling(item), |node| {
        tree.parsed.prev_sibling(*node)
    })
    .take_while(|node| {
        tree.is(*node, "attribute_item") || comments.contains(tree.parsed.kind(*node))
    })
    .filter_map(|node| tree.first_child_of(node, &["attribute"]))
}

// `#[test]`, or a path that ends in `::test`, such as `#[tokio::test]`.
fn is_test_attribute(tree: &Tree, attribute: NodeId) -> bool {
    attribute_path(tree, attribute).is_some_and(|path| path == "test" || path.ends_with("::test"))
}

fn is_cfg_test(tree: &Tree, attribute: NodeId) -> bool {
    let arguments = tree
        .field(attribute, Field::Arguments)
        .map(|arguments| tree.text(arguments));
    attribute_path(tree, attribute) == Some("cfg")
        && arguments
            .is_some_and(|arguments| arguments.split_whitespace().collect::<String>() == "(test)")
}

fn attribute_path<'a>(tree: &Tree<'a>, attribute: NodeId) -> Option<&'a str> {
    tree.named_children(attribute)
        .next()
        .map(|path| tree.text(path))
}

fn adt(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let name_node = tree.field(node, Field::Name)?;
    let name = tree.text(name_node);
    let kind = tree.kind(node);
    let (_, symbol_kind, keyword) = ADTS.iter().find(|(adt, _, _)| *adt == kind)?;
    Some(
        Symbol::at(tree, node, name, *symbol_kind)
            .named_by(tree, Some(name_node))
            .signed(&format!("{keyword} {name}"))
            .public(is_public(tree, node)),
    )
}

// The functions of an impl block's body, owned by the type the block implements. The methods of
// `impl Trait for T` fill the trait's protocol.
fn methods(tree: &Tree, node: NodeId) -> Vec<Symbol> {
    let owner = tree.impl_owner(node).map_or("", |owner| tree.text(owner));
    let protocol = tree.field(node, Field::Trait).is_some();
    let Some(body) = tree.field(node, Field::Body) else {
        return Vec::new();
    };
    tree.children(body)
        .filter(|child| tree.is(*child, "function_item"))
        .map(|child| function(tree, child, owner).in_protocol(protocol))
        .collect()
}

fn constant(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let name = tree.text(tree.field(node, Field::Name)?);
    let typed = tree
        .field(node, Field::Type)
        .map_or(String::new(), |typed| format!(": {}", tree.text(typed)));
    let value = tree
        .field(node, Field::Value)
        .map_or("", |value| tree.text(value));
    let value = truncate(value, MAX_VALUE_CHARS);
    Some(
        Symbol::at(tree, node, name, SymbolKind::Constant)
            .signed(&format!("const {name}{typed} = {value}"))
            .public(is_public(tree, node)),
    )
}

fn type_alias(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let name = tree.text(tree.field(node, Field::Name)?);
    Some(
        Symbol::at(tree, node, name, SymbolKind::TypeAlias)
            .signed(&truncate(tree.text(node), MAX_TYPE_ALIAS_CHARS))
            .public(is_public(tree, node)),
    )
}
