//! The Rust items: functions, structs, enums, traits, the methods of impl blocks, constants and
//! type aliases, in modules at any depth.

use syntax::{NodeId, rust_impl_owner};

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

pub(super) fn symbols(tree: &Tree) -> Vec<Symbol> {
    tree.each_declaration(&ITEMS, |node| item(tree, node))
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
    let name = tree
        .field(node, Field::Name)
        .map_or(UNKNOWN, |name| tree.text(name));
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
        .signed(&format!("{prefix} {name}{parameters}{returns}"))
        .owned_by(parent)
        .public(is_public(tree, node))
}

fn adt(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let name = tree.text(tree.field(node, Field::Name)?);
    let kind = tree.kind(node);
    let (_, symbol_kind, keyword) = ADTS.iter().find(|(adt, _, _)| *adt == kind)?;
    Some(
        Symbol::at(tree, node, name, *symbol_kind)
            .signed(&format!("{keyword} {name}"))
            .public(is_public(tree, node)),
    )
}

// The functions of an impl block's body, owned by the type the block implements.
fn methods(tree: &Tree, node: NodeId) -> Vec<Symbol> {
    let owner = rust_impl_owner(tree.parsed, node).map_or("", |owner| tree.text(owner));
    let Some(body) = tree.field(node, Field::Body) else {
        return Vec::new();
    };
    tree.children(body)
        .filter(|child| tree.is(*child, "function_item"))
        .map(|child| function(tree, child, owner))
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
