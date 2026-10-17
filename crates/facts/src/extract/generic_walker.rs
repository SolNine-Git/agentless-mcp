//! The generic walker: the symbols of each language whose declarations a kind table describes.
//! The walk enters every node that is no declaration, stops at a function, a function-valued
//! binding or a constant, and reads a class through its body, under the class's name.

use std::rc::Rc;

use syntax::{Declarations, Extraction, NodeId};

use super::conventionally_public;
use super::tree::{Field, Tree};
use crate::values::{Symbol, SymbolKind};

const NON_PUBLIC_MODIFIERS: [&str; 4] = ["private", "protected", "internal", "fileprivate"];
const KEYWORD_SUFFIXES: [&str; 3] = ["_declaration", "_definition", "_spec"];

pub(super) fn symbols(tree: &Tree, extraction: &Extraction) -> Vec<Symbol> {
    let walk = Walk {
        tree,
        extraction,
        declarations: &extraction.declarations,
    };
    let mut symbols = Vec::new();
    let mut frames = vec![(tree.children(tree.root()), Rc::<str>::from(""))];
    while let Some((children, owner)) = frames.last_mut() {
        let Some(node) = children.next() else {
            frames.pop();
            continue;
        };
        let owner = Rc::clone(owner);
        let visit = walk.visit(node, &owner);
        symbols.extend(visit.symbol);
        if let Some((parent, owner)) = visit.next {
            frames.push((tree.children(parent), owner));
        }
    }
    symbols
}

struct Walk<'t, 'a> {
    tree: &'t Tree<'a>,
    extraction: &'t Extraction,
    declarations: &'t Declarations,
}

// What one node gives: a symbol, and the node whose children the walk visits next, with their
// owner.
struct Visit {
    symbol: Option<Symbol>,
    next: Option<(NodeId, Rc<str>)>,
}

impl Walk<'_, '_> {
    fn visit(&self, node: NodeId, owner: &Rc<str>) -> Visit {
        let kind = self.tree.parsed.kind(node);
        let declarations = self.declarations;
        if declarations.functions.contains(kind) {
            return Visit::stop(self.function(node, owner));
        }
        if declarations.bindings.contains(kind) {
            return self.binding(node, owner);
        }
        if declarations.constants.contains(kind) {
            return Visit::stop(self.constant(node, owner));
        }
        if declarations.classes.contains(kind) {
            return self.class(node, owner);
        }
        Visit::descend(None, node, Rc::clone(owner))
    }

    // A function owned by its class, else by the type its receiver names.
    fn function(&self, node: NodeId, owner: &str) -> Option<Symbol> {
        let name = self.name(node)?;
        let owner = if owner.is_empty() {
            self.receiver_owner(node)
        } else {
            owner
        };
        Some(self.function_symbol(node, node, name, owner))
    }

    fn function_symbol(&self, node: NodeId, header: NodeId, name: &str, owner: &str) -> Symbol {
        let kind = if owner.is_empty() {
            SymbolKind::Function
        } else {
            SymbolKind::Method
        };
        Symbol::at(self.tree, node, name, kind)
            .signed(&self.signature(header, name))
            .owned_by(owner)
            .public(self.is_public(node, name))
    }

    // `const f = () => {}`: the binding names a function whose header is the value. A binding of
    // any other value is no declaration, so the walk goes on through it.
    fn binding(&self, node: NodeId, owner: &Rc<str>) -> Visit {
        let value = self.tree.field(node, Field::Value).filter(|value| {
            self.declarations
                .function_values
                .contains(self.tree.parsed.kind(*value))
        });
        let Some(value) = value else {
            return Visit::descend(None, node, Rc::clone(owner));
        };
        let symbol = self
            .name(node)
            .map(|name| self.function_symbol(node, value, name, owner));
        Visit::stop(symbol)
    }

    // A constant carries every keyword that the table requires, `static final` in Java.
    fn constant(&self, node: NodeId, owner: &str) -> Option<Symbol> {
        let keywords = self.modifier_keywords(node);
        let required = self.declarations.constant_keywords;
        if !required.iter().all(|keyword| keywords.contains(keyword)) {
            return None;
        }
        let declarator = self.tree.field(node, Field::Declarator);
        let name = self.name(declarator.unwrap_or(node))?;
        Some(
            Symbol::at(self.tree, node, name, SymbolKind::Constant)
                .signed(self.header_text(node))
                .owned_by(owner)
                .public(self.is_public(node, name)),
        )
    }

    fn class(&self, node: NodeId, owner: &Rc<str>) -> Visit {
        let Some(name) = self.name(node) else {
            return Visit::stop(None);
        };
        let symbol = Symbol::at(self.tree, node, name, SymbolKind::Class)
            .signed(&format!("{} {name}", self.keyword(node)))
            .owned_by(owner)
            .public(self.is_public(node, name));
        let body = self.class_body(node);
        Visit {
            symbol: Some(symbol),
            next: body.map(|body| (body, Rc::from(name))),
        }
    }

    // The name field's text, else the first child of a naming kind; an empty name is none.
    fn name(&self, node: NodeId) -> Option<&str> {
        let tree = self.tree;
        let field = self.declarations.name_field;
        let named = field.and_then(|field| tree.child_by_field(node, field));
        let text = match named {
            Some(named) => tree.text(named),
            None => tree
                .first_child_in(node, &self.declarations.names)
                .map_or("", |child| tree.text(child)),
        };
        (!text.is_empty()).then_some(text)
    }

    // The first type identifier in the receiver: `(c *Config)` and `(s *Stack[T])` nest it.
    fn receiver_owner(&self, node: NodeId) -> &str {
        let tree = self.tree;
        let receiver = self
            .declarations
            .receiver_field
            .and_then(|field| tree.child_by_field(node, field));
        receiver
            .and_then(|receiver| {
                tree.preorder(receiver)
                    .find(|child| tree.is(*child, "type_identifier"))
            })
            .map_or("", |owner| tree.text(owner))
    }

    // The keyword that opened a type declaration: the declaration_kind field where the grammar
    // has one, else the node kind without its suffixes.
    fn keyword(&self, node: NodeId) -> &str {
        if let Some(spelled) = self.tree.field(node, Field::DeclarationKind) {
            return self.tree.text(spelled);
        }
        KEYWORD_SUFFIXES
            .iter()
            .fold(self.tree.kind(node), |kind, suffix| {
                kind.strip_suffix(suffix).unwrap_or(kind)
            })
    }

    fn class_body(&self, node: NodeId) -> Option<NodeId> {
        self.tree.field(node, Field::Body).or_else(|| {
            self.tree
                .first_child_in(node, &self.declarations.class_bodies)
        })
    }

    fn modifier_keywords(&self, node: NodeId) -> Vec<&str> {
        let tree = self.tree;
        tree.children(node)
            .filter(|child| {
                self.declarations
                    .modifiers
                    .contains(tree.parsed.kind(*child))
            })
            .flat_map(|child| text::split_whitespace(tree.text(child)))
            .collect()
    }

    fn is_public(&self, node: NodeId, name: &str) -> bool {
        if !self.declarations.keyword_visibility {
            return conventionally_public(name);
        }
        let keywords = self.modifier_keywords(node);
        !NON_PUBLIC_MODIFIERS
            .iter()
            .any(|modifier| keywords.contains(modifier))
    }

    // `fn name(params) -> result`, or the header text for a grammar with no parameters field.
    fn signature(&self, header: NodeId, name: &str) -> String {
        if self.declarations.signature_from_header {
            return self.header_text(header).to_owned();
        }
        let tree = self.tree;
        let parameters = tree
            .field(header, Field::Parameters)
            .map_or("()", |parameters| tree.text(parameters));
        let result = tree
            .field(header, Field::ReturnType)
            .or_else(|| tree.field(header, Field::Result))
            .map_or("", |result| {
                let text = tree.text(result);
                text::strip(text.strip_prefix(':').unwrap_or(text))
            });
        let arrow = if result.is_empty() {
            String::new()
        } else {
            format!(" -> {result}")
        };
        format!("fn {name}{parameters}{arrow}")
    }

    // A declaration's own text up to the start of its body.
    fn header_text(&self, node: NodeId) -> &str {
        let tree = self.tree;
        let body = tree
            .field(node, Field::Body)
            .or_else(|| tree.first_child_in(node, &self.extraction.bodies));
        let end = body.map_or_else(
            || tree.parsed.end_byte(node),
            |body| tree.parsed.start_byte(body),
        );
        let start = tree.parsed.start_byte(node) as usize;
        text::strip(&tree.parsed.source()[start..end as usize])
    }
}

impl Visit {
    fn stop(symbol: Option<Symbol>) -> Visit {
        Visit { symbol, next: None }
    }

    fn descend(symbol: Option<Symbol>, node: NodeId, owner: Rc<str>) -> Visit {
        Visit {
            symbol,
            next: Some((node, owner)),
        }
    }
}
