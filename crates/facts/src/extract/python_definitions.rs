//! The Python symbols: functions, classes with their methods, nested functions, upper-case
//! module constants and type aliases.

use syntax::NodeId;

use super::tree::{Field, Tree};
use super::{conventionally_public, declares_async};
use crate::pattern::upper_case_line;
use crate::spelling::{base_name, truncate};
use crate::values::{Symbol, SymbolKind};

const DECLARATIONS: [&str; 6] = [
    "function_definition",
    "class_definition",
    "expression_statement",
    "assignment",
    "type_alias_statement",
    "decorated_definition",
];
// What ends the descent in a function body: a class is a stop, so that its methods never join
// the function's chain.
const NESTED_STOPS: [&str; 3] = [
    "function_definition",
    "decorated_definition",
    "class_definition",
];
const DATACLASS_DECORATORS: [&str; 2] = ["dataclass", "dataclasses.dataclass"];
const ENUM_BASES: [&str; 3] = ["Enum", "IntEnum", "StrEnum"];
const MAX_VALUE_CHARS: usize = 80;
const MAX_TYPE_ALIAS_CHARS: usize = 120;
const UNKNOWN: &str = "<unknown>";

// One step of the walk. The source sets how deep definitions nest, so the walk keeps its own
// stack instead of recursing.
enum Work {
    Declaration(NodeId),
    // A function, then the functions nested in it.
    Function {
        node: NodeId,
        parent: String,
    },
    // The functions nested in the body of `node`, under the chain `parent`.
    Nested {
        node: NodeId,
        parent: String,
    },
    // A nested function, which stays a function under a class, then its own nested functions.
    NestedFunction {
        node: NodeId,
        parent: String,
    },
    // A class with the decorators of a wrapping definition, then its members.
    Class {
        node: NodeId,
        decorators: Vec<String>,
    },
    Member {
        node: NodeId,
        class: String,
    },
    Decorated {
        node: NodeId,
        parent: String,
    },
}

// Each step emits its own symbol before the steps it starts, so the order is the preorder of the
// definitions.
pub(super) fn symbols(tree: &Tree) -> Vec<Symbol> {
    let declarations =
        tree.declarations_under(tree.root(), |node| tree.is_any(node, &DECLARATIONS));
    let mut pending: Vec<Work> = declarations
        .into_iter()
        .rev()
        .map(Work::Declaration)
        .collect();
    let mut symbols = Vec::new();
    while let Some(work) = pending.pop() {
        let (symbol, next) = step(tree, work);
        symbols.extend(symbol);
        pending.extend(next.into_iter().rev());
    }
    symbols
}

fn step(tree: &Tree, work: Work) -> (Option<Symbol>, Vec<Work>) {
    match work {
        Work::Declaration(node) => declaration(tree, node),
        Work::Function { node, parent } => {
            let symbol = function(tree, node, &parent);
            let chain = qualified(&parent, &symbol.name);
            (
                Some(symbol),
                vec![Work::Nested {
                    node,
                    parent: chain,
                }],
            )
        }
        Work::Nested { node, parent } => (None, nested(tree, node, &parent)),
        Work::NestedFunction { node, parent } => {
            let symbol = function(tree, node, &parent).of_kind(SymbolKind::Function);
            let chain = format!("{parent}.{}", symbol.name);
            (
                Some(symbol),
                vec![Work::Nested {
                    node,
                    parent: chain,
                }],
            )
        }
        Work::Class { node, decorators } => class(tree, node, &decorators),
        Work::Member { node, class } => (None, member(tree, node, class)),
        Work::Decorated { node, parent } => (None, decorated(tree, node, parent)),
    }
}

fn declaration(tree: &Tree, node: NodeId) -> (Option<Symbol>, Vec<Work>) {
    let parent = String::new();
    match tree.kind(node) {
        "function_definition" => (None, vec![Work::Function { node, parent }]),
        "class_definition" => (
            None,
            vec![Work::Class {
                node,
                decorators: Vec::new(),
            }],
        ),
        "type_alias_statement" => (type_alias(tree, node), Vec::new()),
        "decorated_definition" => (None, vec![Work::Decorated { node, parent }]),
        _ => (constant(tree, node), Vec::new()),
    }
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
        "async def"
    } else {
        "def"
    };
    let kind = if parent.is_empty() {
        SymbolKind::Function
    } else {
        SymbolKind::Method
    };
    Symbol::at(tree, node, name, kind)
        .signed(&format!("{prefix} {name}{parameters}{returns}"))
        .owned_by(parent)
        .public(conventionally_public(name))
}

// Each `def` in a function body, as a function whose parent is the chain of the functions
// around it; a class in the body ends the descent.
fn nested(tree: &Tree, node: NodeId, parent: &str) -> Vec<Work> {
    let Some(body) = tree.field(node, Field::Body) else {
        return Vec::new();
    };
    let stops = tree.declarations_under(body, |child| tree.is_any(child, &NESTED_STOPS));
    stops
        .into_iter()
        .filter_map(|child| nested_definition(tree, child))
        .map(|node| Work::NestedFunction {
            node,
            parent: parent.to_owned(),
        })
        .collect()
}

fn nested_definition(tree: &Tree, node: NodeId) -> Option<NodeId> {
    match tree.kind(node) {
        "decorated_definition" => tree.first_child_of(node, &["function_definition"]),
        "function_definition" => Some(node),
        _ => None,
    }
}

fn decorated(tree: &Tree, node: NodeId, parent: String) -> Vec<Work> {
    let definition = tree.first_child_of(node, &["function_definition", "class_definition"]);
    match definition {
        Some(function) if tree.is(function, "function_definition") => vec![Work::Function {
            node: function,
            parent,
        }],
        Some(class_node) => vec![Work::Class {
            node: class_node,
            decorators: decorators(tree, node),
        }],
        None => Vec::new(),
    }
}

// A class, then the steps of its members; `extra` are the decorators that a wrapping definition
// carries.
fn class(tree: &Tree, node: NodeId, extra: &[String]) -> (Option<Symbol>, Vec<Work>) {
    let name = tree
        .field(node, Field::Name)
        .map_or(UNKNOWN, |name| tree.text(name));
    let bases = bases(tree, node);
    let own;
    let decorators = if extra.is_empty() {
        own = own_decorators(tree, node);
        &own
    } else {
        extra
    };
    let superclasses = tree
        .field(node, Field::Superclasses)
        .map_or("", |superclasses| tree.text(superclasses));
    let symbol = Symbol::at(tree, node, name, class_kind(&bases, decorators))
        .signed(&format!("class {name}{superclasses}"))
        .public(conventionally_public(name))
        .based_on(bases);
    let members = tree.field(node, Field::Body).map_or_else(Vec::new, |body| {
        tree.children(body)
            .map(|child| Work::Member {
                node: child,
                class: name.to_owned(),
            })
            .collect()
    });
    (Some(symbol), members)
}

// A method of a class body, and a decorated definition there.
fn member(tree: &Tree, node: NodeId, class: String) -> Vec<Work> {
    match tree.kind(node) {
        "function_definition" => vec![Work::Function {
            node,
            parent: class,
        }],
        "decorated_definition" => decorated(tree, node, class),
        _ => Vec::new(),
    }
}

fn class_kind(bases: &[String], decorators: &[String]) -> SymbolKind {
    let has_decorator = |name: &str| {
        decorators
            .iter()
            .any(|decorator| decorator.split('(').next() == Some(name))
    };
    let has_base = |names: &[&str]| bases.iter().any(|base| names.contains(&base_name(base)));
    if DATACLASS_DECORATORS.into_iter().any(has_decorator) {
        SymbolKind::Dataclass
    } else if has_base(&["Protocol"]) {
        SymbolKind::Protocol
    } else if has_base(&ENUM_BASES) {
        SymbolKind::Enum
    } else {
        SymbolKind::Class
    }
}

fn bases(tree: &Tree, node: NodeId) -> Vec<String> {
    tree.field(node, Field::Superclasses)
        .map(|superclasses| {
            tree.named_children(superclasses)
                .map(|base| tree.text(base).to_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn own_decorators(tree: &Tree, node: NodeId) -> Vec<String> {
    tree.parsed
        .parent(node)
        .filter(|parent| tree.is(*parent, "decorated_definition"))
        .map(|parent| decorators(tree, parent))
        .unwrap_or_default()
}

fn decorators(tree: &Tree, node: NodeId) -> Vec<String> {
    tree.children(node)
        .filter(|child| tree.is(*child, "decorator"))
        .map(|child| {
            let text = tree.text(child);
            text.strip_prefix('@').unwrap_or(text).to_owned()
        })
        .collect()
}

// An upper-case module constant: the assignment itself, or the first child of a statement.
fn constant(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let assignment = if tree.is(node, "assignment") {
        node
    } else {
        tree.children(node).next()?
    };
    if !tree.is(assignment, "assignment") {
        return None;
    }
    let left = tree
        .field(assignment, Field::Left)
        .filter(|left| tree.is(*left, "identifier"))?;
    let name = tree.text(left);
    if !upper_case_line(name) {
        return None;
    }
    let value = tree
        .field(assignment, Field::Right)
        .map_or("", |right| tree.text(right));
    let value = truncate(value, MAX_VALUE_CHARS);
    Some(
        Symbol::at(tree, node, name, SymbolKind::Constant)
            .signed(&format!("{name} = {value}"))
            .public(conventionally_public(name)),
    )
}

fn type_alias(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let name = tree.text(tree.field(node, Field::Name)?);
    Some(
        Symbol::at(tree, node, name, SymbolKind::TypeAlias)
            .signed(&truncate(tree.text(node), MAX_TYPE_ALIAS_CHARS))
            .public(conventionally_public(name)),
    )
}

fn qualified(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}.{name}")
    }
}
