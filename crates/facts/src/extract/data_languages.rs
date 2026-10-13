//! The data languages: the keys of JSON, TOML and YAML documents, HCL blocks and their
//! attributes, and SQL tables, views and columns.

use std::rc::Rc;

use syntax::{Language, NodeId};

use super::tree::{Field, Tree};
use crate::values::{Symbol, SymbolKind};

const PAIRS: [&str; 2] = ["pair", "block_mapping_pair"];
const TOML_TABLES: [&str; 2] = ["table", "table_array_element"];
const OWNERS: [&str; 4] = ["pair", "block_mapping_pair", "table", "table_array_element"];
const KEYS: [&str; 3] = ["bare_key", "quoted_key", "dotted_key"];
const CONTAINERS: [&str; 4] = ["object", "array", "block_node", "flow_node"];
const QUOTES: [char; 2] = ['"', '\''];

// TOML tables first, then every pair under the keys of the pairs and tables around it.
pub(super) fn keys(tree: &Tree) -> Vec<Symbol> {
    let language = tree.parsed.language();
    let mut symbols = if language == Language::Toml {
        tree.each_where(
            |node| tree.is_any(node, &TOML_TABLES),
            |node| table(tree, node),
        )
    } else {
        Vec::new()
    };
    symbols.extend(pairs(tree, language));
    symbols
}

fn table(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let key = data_key(tree, node, Language::Toml);
    if key.is_empty() {
        return None;
    }
    let (parent, name) = key.rsplit_once('.').unwrap_or(("", key));
    Some(
        Symbol::at(tree, node, name, SymbolKind::Class)
            .signed(&format!("table {key}"))
            .owned_by(parent),
    )
}

// One downward walk that carries the owner it derived: re-deriving it per pair costs a deep
// document far more than its size.
fn pairs(tree: &Tree, language: Language) -> Vec<Symbol> {
    let pair = if language == Language::Yaml {
        "block_mapping_pair"
    } else {
        "pair"
    };
    let mut symbols = Vec::new();
    let mut frames = vec![(tree.children(tree.root()), Rc::<str>::from(""))];
    while let Some((children, owner)) = frames.last_mut() {
        let Some(node) = children.next() else {
            frames.pop();
            continue;
        };
        let owner = Rc::clone(owner);
        if !tree.is_any(node, &OWNERS) {
            frames.push((tree.children(node), owner));
            continue;
        }
        let key = data_key(tree, node, language);
        frames.push((tree.children(node), nested_owner(&owner, key)));
        if tree.is(node, pair) && !key.is_empty() {
            symbols.push(pair_symbol(tree, node, key, &owner));
        }
    }
    symbols
}

fn nested_owner(owner: &str, key: &str) -> Rc<str> {
    match (owner.is_empty(), key.is_empty()) {
        (_, true) => Rc::from(owner),
        (true, false) => Rc::from(key),
        (false, false) => Rc::from(format!("{owner}.{key}")),
    }
}

// A pair whose value is a container holding pairs is a class; any other pair is a constant.
fn pair_symbol(tree: &Tree, node: NodeId, key: &str, owner: &str) -> Symbol {
    let value = tree.field(node, Field::Value);
    let nested = value.is_some_and(|value| {
        tree.is_any(value, &CONTAINERS)
            && tree
                .preorder(value)
                .skip(1)
                .any(|inner| tree.is_any(inner, &PAIRS))
    });
    let kind = if nested {
        SymbolKind::Class
    } else {
        SymbolKind::Constant
    };
    Symbol::at(tree, node, key, kind)
        .signed(tree.text(node))
        .owned_by(owner)
}

// The key that one pair or TOML table declares, with one pair of quotes off in JSON and TOML.
fn data_key<'a>(tree: &Tree<'a>, node: NodeId, language: Language) -> &'a str {
    let key = tree.field(node, Field::Key).or_else(|| {
        tree.named_children(node)
            .find(|child| tree.is_any(*child, &KEYS))
    });
    key.map_or("", |key| clean_name(tree.text(key), language))
}

// A key or label without its whitespace, and without one pair of matching quotes in the
// languages whose keys and labels are quoted strings.
fn clean_name(text: &str, language: Language) -> &str {
    let cleaned = text::strip(text);
    let quoted = matches!(language, Language::Json | Language::Hcl | Language::Toml)
        && cleaned.len() >= 2
        && QUOTES
            .iter()
            .any(|quote| cleaned.starts_with(*quote) && cleaned.ends_with(*quote));
    if quoted {
        &cleaned[1..cleaned.len() - 1]
    } else {
        cleaned
    }
}

// Each block, named by its labels, and each attribute, owned by the block around it.
pub(super) fn hcl(tree: &Tree) -> Vec<Symbol> {
    tree.each_where(
        |node| tree.is_any(node, &["block", "attribute"]),
        |node| {
            if tree.is(node, "block") {
                hcl_block(tree, node)
            } else {
                hcl_attribute(tree, node)
            }
        },
    )
}

fn hcl_block(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let identity = block_identity(tree, node).filter(|identity| !identity.name.is_empty())?;
    let header = tree.text(node).split('{').next().unwrap_or_default();
    Some(
        Symbol::at(tree, node, identity.name, SymbolKind::Class)
            .signed(text::strip(header))
            .owned_by(&identity.owner),
    )
}

fn hcl_attribute(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let name = tree
        .named_children(node)
        .find(|child| tree.is(*child, "identifier"))?;
    let owner = std::iter::successors(tree.parsed.parent(node), |parent| {
        tree.parsed.parent(*parent)
    })
    .find(|parent| tree.is(*parent, "block"))
    .and_then(|block| block_identity(tree, block))
    .map_or(String::new(), |identity| {
        [identity.owner.as_str(), identity.name]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<&str>>()
            .join(".")
    });
    Some(
        Symbol::at(tree, node, tree.text(name), SymbolKind::Constant)
            .signed(tree.text(node))
            .owned_by(&owner),
    )
}

// A block's labels: the last is its name and the ones before it its owner.
struct Identity<'a> {
    owner: String,
    name: &'a str,
}

fn block_identity<'a>(tree: &Tree<'a>, node: NodeId) -> Option<Identity<'a>> {
    let parts: Vec<&str> = tree
        .named_children(node)
        .filter(|child| tree.is_any(*child, &["identifier", "string_lit"]))
        .map(|child| clean_name(tree.text(child), Language::Hcl))
        .collect();
    let (name, labels) = parts.split_last()?;
    Some(Identity {
        owner: labels.join("."),
        name,
    })
}

// Each table and view of a CREATE statement, then the columns of each table.
pub(super) fn sql(tree: &Tree) -> Vec<Symbol> {
    let statements = tree.each_where(
        |node| tree.is_any(node, &["create_table", "create_view"]),
        Some,
    );
    statements
        .into_iter()
        .flat_map(|node| sql_statement(tree, node))
        .collect()
}

fn sql_statement(tree: &Tree, node: NodeId) -> Vec<Symbol> {
    let Some(name) = object_name(tree, node) else {
        return Vec::new();
    };
    let table = tree.is(node, "create_table");
    let kind = if table {
        SymbolKind::Class
    } else {
        SymbolKind::TypeAlias
    };
    let symbol = Symbol::at(tree, node, name, kind).signed(tree.text(node));
    let mut symbols = vec![symbol];
    if table {
        symbols.extend(columns(tree, node, name));
    }
    symbols
}

fn columns(tree: &Tree, node: NodeId, table: &str) -> Vec<Symbol> {
    tree.preorder(node)
        .filter(|child| tree.is(*child, "column_definition"))
        .filter_map(|child| {
            let column = tree.text(tree.field(child, Field::Name)?);
            Some(
                Symbol::at(tree, child, column, SymbolKind::Constant)
                    .signed(tree.text(child))
                    .owned_by(table),
            )
        })
        .collect()
}

// The name a CREATE statement declares directly: its object reference's name field, or the
// whole reference.
fn object_name<'a>(tree: &Tree<'a>, node: NodeId) -> Option<&'a str> {
    let reference = tree
        .named_children(node)
        .find(|child| tree.is(*child, "object_reference"))?;
    let name = tree.field(reference, Field::Name).unwrap_or(reference);
    let text = tree.text(name);
    (!text.is_empty()).then_some(text)
}
