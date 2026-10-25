//! The data languages: the keys of JSON, TOML and YAML documents, HCL blocks and their
//! attributes, and SQL tables, views and columns.

use std::rc::Rc;

use syntax::{Language, NodeId};

use super::tree::{Field, Tree};
use crate::spelling::truncate;
use crate::values::{Symbol, SymbolKind, qualified};

const PAIRS: [&str; 2] = ["pair", "block_mapping_pair"];
const TOML_TABLES: [&str; 2] = ["table", "table_array_element"];
const KEYS: [&str; 3] = ["bare_key", "quoted_key", "dotted_key"];
const CONTAINERS: [&str; 4] = ["object", "array", "block_node", "flow_node"];
const KEY_QUOTES: [char; 2] = ['"', '\''];
const LISTS: [&str; 3] = ["array", "block_sequence", "flow_sequence"];
const MAPPINGS: [&str; 4] = ["object", "inline_table", "block_mapping", "flow_mapping"];
const YAML_NODES: [&str; 2] = ["block_node", "flow_node"];
const YAML_PROPERTIES: [&str; 2] = ["anchor", "tag"];
// The deepest level an outline lists: a key of the root mapping is level 1.
const OUTLINE_DEPTH: u8 = 2;
const PREVIEW_CHARS: usize = 40;

/// The most keys that the outline of one data file lists.
pub const OUTLINE_CAP: usize = 500;

/// The outline of one data file: the keys it lists with their levels, the count of every key
/// in the file, and the shape of a root that is not a mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataOutline {
    pub rows: Vec<OutlineRow>,
    pub keys: usize,
    pub root: Option<String>,
}

/// One listed key and its level, 1 for a key of the root mapping or a TOML table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineRow {
    pub depth: u8,
    pub symbol: Symbol,
}

pub(super) fn keys(tree: &Tree) -> Vec<Symbol> {
    let rows = outline(tree).rows;
    rows.into_iter().map(|row| row.symbol).collect()
}

// TOML tables first, then the pairs, each in document order.
pub(super) fn outline(tree: &Tree) -> DataOutline {
    let walked = walk(tree, tree.parsed.language());
    let mut rows = walked.tables;
    rows.extend(walked.pairs);
    DataOutline {
        rows: capped(rows),
        keys: walked.keys,
        root: root_shape(tree),
    }
}

// The rows past the cap go: every level-1 row first, then level-2 rows in document order.
fn capped(rows: Vec<OutlineRow>) -> Vec<OutlineRow> {
    if rows.len() <= OUTLINE_CAP {
        return rows;
    }
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|&at| (rows[at].depth, rows[at].symbol.line, at));
    let mut kept = vec![false; rows.len()];
    for at in order.into_iter().take(OUTLINE_CAP) {
        kept[at] = true;
    }
    let rows = rows.into_iter().zip(kept);
    rows.filter_map(|(row, keep)| keep.then_some(row)).collect()
}

// Where a node sits: the owner its keys qualify with, its level, and whether a list holds it.
#[derive(Clone)]
struct Level {
    owner: Rc<str>,
    depth: u8,
    in_list: bool,
}

#[derive(Default)]
struct Walked {
    tables: Vec<OutlineRow>,
    pairs: Vec<OutlineRow>,
    keys: usize,
}

// One downward walk that carries the level it derived: re-deriving it per pair costs a deep
// document far more than its size. Every key counts, and only the outline keys become rows.
fn walk(tree: &Tree, language: Language) -> Walked {
    let pair = if language == Language::Yaml {
        "block_mapping_pair"
    } else {
        "pair"
    };
    let mut walked = Walked::default();
    let top = Level {
        owner: Rc::from(""),
        depth: 0,
        in_list: false,
    };
    let mut frames = vec![(tree.children(tree.root()), top)];
    while let Some((children, level)) = frames.last_mut() {
        let Some(node) = children.next() else {
            frames.pop();
            continue;
        };
        let level = level.clone();
        let inner = if tree.is_any(node, &LISTS) {
            Level {
                in_list: true,
                ..level
            }
        } else if language == Language::Toml && tree.is_any(node, &TOML_TABLES) {
            walked.table(tree, node)
        } else if tree.is(node, pair) {
            walked.pair(tree, node, language, &level)
        } else {
            level
        };
        frames.push((tree.children(node), inner));
    }
    walked
}

impl Walked {
    // A TOML table header is level 1 and its pairs level 2, whatever its dotted key spells.
    fn table(&mut self, tree: &Tree, node: NodeId) -> Level {
        let key = data_key(tree, node, Language::Toml);
        if !key.is_empty() {
            self.keys += 1;
            let (parent, name) = key.rsplit_once('.').unwrap_or(("", key));
            let pairs = tree
                .named_children(node)
                .filter(|child| tree.is(*child, "pair"));
            let signature = format!("table {key}, {}", counted(pairs.count(), "key", "keys"));
            let symbol = Symbol::at(tree, node, name, SymbolKind::Class)
                .signed(&signature)
                .owned_by(parent);
            self.tables.push(OutlineRow { depth: 1, symbol });
        }
        Level {
            owner: nested_owner("", key),
            depth: 1,
            in_list: false,
        }
    }

    fn pair(&mut self, tree: &Tree, node: NodeId, language: Language, level: &Level) -> Level {
        let key = data_key(tree, node, language);
        let depth = level.depth.saturating_add(1);
        if !key.is_empty() {
            self.keys += 1;
            if depth <= OUTLINE_DEPTH && !level.in_list {
                let symbol = pair_symbol(tree, node, key, &level.owner);
                self.pairs.push(OutlineRow { depth, symbol });
            }
        }
        Level {
            owner: nested_owner(&level.owner, key),
            depth,
            in_list: level.in_list,
        }
    }
}

fn nested_owner(owner: &str, key: &str) -> Rc<str> {
    if key.is_empty() {
        return Rc::from(owner);
    }
    Rc::from(qualified(owner, key))
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
    let shape = pair_value(tree, node).map_or_else(String::new, |value| shape(tree, value));
    Symbol::at(tree, node, key, kind)
        .signed(&format!("{key} = {shape}"))
        .owned_by(owner)
}

// The value field, or in TOML, whose pairs name no field, the last child that is not a key.
fn pair_value(tree: &Tree, node: NodeId) -> Option<NodeId> {
    tree.field(node, Field::Value).or_else(|| {
        let values = tree
            .named_children(node)
            .filter(|child| !tree.is_any(*child, &KEYS));
        values.last()
    })
}

// A mapping or a list by its item count, any other value by its text cut short.
fn shape(tree: &Tree, value: NodeId) -> String {
    let inner = unwrapped(tree, value);
    if tree.is_any(inner, &MAPPINGS) {
        format!(
            "object, {}",
            counted(items(tree, inner).count(), "key", "keys")
        )
    } else if tree.is_any(inner, &LISTS) {
        format!(
            "list, {}",
            counted(items(tree, inner).count(), "item", "items")
        )
    } else {
        let words: Vec<&str> = text::split_whitespace(tree.text(value)).collect();
        truncate(&words.join(" "), PREVIEW_CHARS)
    }
}

// The named children that are not comments.
fn items<'a>(tree: &Tree<'a>, node: NodeId) -> impl Iterator<Item = NodeId> + 'a {
    let parsed = tree.parsed;
    parsed
        .named_children(node)
        .filter(move |child| !parsed.kind_name(*child).contains("comment"))
}

fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

// The shape of a JSON or YAML root that is not a mapping, such as a list of rows. A TOML root
// is always a table.
fn root_shape(tree: &Tree) -> Option<String> {
    let language = tree.parsed.language();
    if language == Language::Toml {
        return None;
    }
    let mut document = tree.root();
    if language == Language::Yaml {
        document = items(tree, document).find(|child| tree.is(*child, "document"))?;
    }
    let value = items(tree, document).next()?;
    (!tree.is_any(unwrapped(tree, value), &MAPPINGS)).then(|| shape(tree, value))
}

// A YAML node's content past its anchor and tag, or the value itself in the other languages.
fn unwrapped(tree: &Tree, value: NodeId) -> NodeId {
    if !tree.is_any(value, &YAML_NODES) {
        return value;
    }
    let mut content = items(tree, value).filter(|child| !tree.is_any(*child, &YAML_PROPERTIES));
    content.next().unwrap_or(value)
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
        && KEY_QUOTES
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
