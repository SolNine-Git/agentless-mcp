//! The C and C++ declarators: functions, prototypes, methods declared in a class body or defined
//! out of line, and the tagged types that own them.

use syntax::NodeId;

use super::tree::{Field, Tree};
use crate::values::{Symbol, SymbolKind};

const TAGGED: [&str; 3] = ["struct_specifier", "class_specifier", "enum_specifier"];
// What ends the descent: a tagged type owns what is inside it, and a member's own declarator is
// never a free function. A namespace is not here, so the walk goes through it.
const DECLARATIONS: [&str; 6] = [
    "function_definition",
    "function_declarator",
    "field_declaration",
    "struct_specifier",
    "class_specifier",
    "enum_specifier",
];
// The nodes a declarator ends at when it names a function; anything else there is the
// function-pointer shape, which declares a variable.
const FUNCTION_NAMES: [&str; 6] = [
    "identifier",
    "field_identifier",
    "type_identifier",
    "qualified_identifier",
    "operator_name",
    "destructor_name",
];
// A malformed tree must not spin the declarator walk; real source stays far below this.
const UNWRAP_LIMIT: usize = 32;

// The declarations of the file, then the members of each tagged type, the last one found first.
pub(super) fn symbols(tree: &Tree) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    let mut pending = vec![(tree.root(), String::new())];
    while let Some((scope, owner)) = pending.pop() {
        for node in tree.declarations_under(scope, |node| tree.is_any(node, &DECLARATIONS)) {
            if !tree.is_any(node, &TAGGED) {
                symbols.extend(function(tree, node, &owner));
                continue;
            }
            let Some(symbol) = tagged(tree, node, &owner) else {
                continue;
            };
            if let Some(body) = tree.field(node, Field::Body) {
                pending.push((body, qualified(&owner, &symbol.name)));
            }
            symbols.push(symbol);
        }
    }
    symbols
}

fn tagged(tree: &Tree, node: NodeId, owner: &str) -> Option<Symbol> {
    let name = tree.text(tree.field(node, Field::Name)?);
    let keyword = tree.kind(node).split('_').next().unwrap_or_default();
    Some(
        Symbol::at(tree, node, name, SymbolKind::Class)
            .signed(&format!("{keyword} {name}"))
            .owned_by(owner),
    )
}

fn function(tree: &Tree, node: NodeId, owner: &str) -> Option<Symbol> {
    let site = function_site(tree, node)?;
    let parent = if site.owner.is_empty() {
        owner
    } else {
        &site.owner
    };
    let kind = if parent.is_empty() {
        SymbolKind::Function
    } else {
        SymbolKind::Method
    };
    Some(
        Symbol::at(tree, node, site.name, kind)
            .signed(&format!("fn {}", site.name))
            .owned_by(parent),
    )
}

// The owner and name that one declarator spells; the owner is filled only by the out-of-line
// form `A::B::method`.
struct Site<'a> {
    owner: String,
    name: &'a str,
}

// A name only reached through a parenthesis and a star declares a variable, unless a nested
// declarator declares a function of its own. The source sets the depth, so this is a loop.
fn function_site<'a>(tree: &Tree<'a>, node: NodeId) -> Option<Site<'a>> {
    let mut declarator = function_declarator(tree, node)?;
    loop {
        let named = tree.field(declarator, Field::Declarator)?;
        if tree.is_any(named, &FUNCTION_NAMES) {
            return Some(qualified_name(tree, named));
        }
        declarator = function_declarator(tree, named)?;
    }
}

fn function_declarator(tree: &Tree, node: NodeId) -> Option<NodeId> {
    let mut current = Some(node);
    for _ in 0..UNWRAP_LIMIT {
        let found = current?;
        if tree.is(found, "function_declarator") {
            return Some(found);
        }
        current = tree.field(found, Field::Declarator).or_else(|| {
            tree.is(found, "parenthesized_declarator")
                .then(|| tree.named_children(found).next())
                .flatten()
        });
    }
    None
}

// `A::B::method` is the owner `A.B` and the name `method`; template arguments leave the owner.
fn qualified_name<'a>(tree: &Tree<'a>, node: NodeId) -> Site<'a> {
    let mut scopes: Vec<&str> = Vec::new();
    let mut current = node;
    while tree.is(current, "qualified_identifier") {
        let Some(following) = tree.field(current, Field::Name) else {
            break;
        };
        let outer = tree.named_children(current).next();
        if let Some(outer) = outer.filter(|outer| *outer != following) {
            scopes.push(tree.text(outer).split('<').next().unwrap_or_default());
        }
        current = following;
    }
    Site {
        owner: scopes.join("."),
        name: tree.text(current),
    }
}

fn qualified(owner: &str, name: &str) -> String {
    if owner.is_empty() {
        name.to_owned()
    } else {
        format!("{owner}.{name}")
    }
}
