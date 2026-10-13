//! The Lua assignments: the root's function declarations, `M.f = function() end`, module table
//! fields, and `local` functions, tables and upper-case constants.

use syntax::NodeId;

use super::conventionally_public;
use super::tree::{Field, Tree};
use crate::pattern::upper_case_line;
use crate::spelling::truncate;
use crate::values::{Symbol, SymbolKind};

const NAMES: [&str; 2] = ["identifier", "dot_index_expression"];
const MAX_VALUE_CHARS: usize = 80;

// Only the root's own children declare: a function body's locals are its own.
pub(super) fn symbols(tree: &Tree) -> Vec<Symbol> {
    tree.children(tree.root())
        .filter_map(|node| match tree.kind(node) {
            "function_declaration" => function(tree, node),
            "assignment_statement" => assignment(tree, node),
            "variable_declaration" => variable(tree, node),
            _ => None,
        })
        .collect()
}

fn function(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let name = tree.text(tree.field(node, Field::Name)?);
    symbol(tree, node, name, SymbolKind::Function, "")
}

// `M.f = function(x) end` is a function, and `M.version = "1.0"` a field of the module table.
fn assignment(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let mut variables = tree.field(node, Field::Variables);
    let mut values = tree.field(node, Field::Values);
    if variables.is_none() {
        let (found_variables, found_values) = lists(tree, node);
        variables = found_variables;
        values = found_values.or(values);
    }
    let name = target_name(tree, variables?)?;
    if values.is_some_and(|values| holds(tree, values, "function_definition")) {
        let parameters = values.map_or("()", |values| function_parameters(tree, values));
        return symbol(
            tree,
            node,
            name,
            SymbolKind::Function,
            &format!("fn {name}{parameters}"),
        );
    }
    let value = values.map_or(String::new(), |values| {
        truncate(tree.text(values), MAX_VALUE_CHARS)
    });
    let signature = if value.is_empty() {
        name.to_owned()
    } else {
        format!("{name} = {value}")
    };
    symbol(tree, node, name, SymbolKind::Constant, &signature)
}

// `local f = function() end`, `local M = {}` and `local LIMIT = 3`.
fn variable(tree: &Tree, node: NodeId) -> Option<Symbol> {
    let assignment = tree.first_child_of(node, &["assignment_statement"])?;
    let (variables, values) = lists(tree, assignment);
    let name = target_name(tree, variables?)?;
    let holding = |kind| values.is_some_and(|values| holds(tree, values, kind));
    if holding("function_definition") {
        let parameters = values.map_or("()", |values| function_parameters(tree, values));
        let signature = format!("local fn {name}{parameters}");
        return symbol(tree, node, name, SymbolKind::Function, &signature);
    }
    if holding("table_constructor") {
        return symbol(
            tree,
            node,
            name,
            SymbolKind::Class,
            &format!("local {name} = {{}}"),
        );
    }
    if upper_case_line(name) {
        return symbol(
            tree,
            node,
            name,
            SymbolKind::Constant,
            &format!("local {name}"),
        );
    }
    None
}

// The last variable list and the last expression list among the children.
fn lists(tree: &Tree, node: NodeId) -> (Option<NodeId>, Option<NodeId>) {
    let last = |kind| {
        tree.children(node)
            .filter(|child| tree.is(*child, kind))
            .last()
    };
    (last("variable_list"), last("expression_list"))
}

fn target_name<'a>(tree: &Tree<'a>, variables: NodeId) -> Option<&'a str> {
    let name = tree.text(tree.first_child_of(variables, &NAMES)?);
    (!name.is_empty()).then_some(name)
}

fn holds(tree: &Tree, values: NodeId, kind: &str) -> bool {
    tree.children(values).any(|child| tree.is(child, kind))
}

// The parameters of the first assigned function that has them, else `()`.
fn function_parameters<'a>(tree: &Tree<'a>, values: NodeId) -> &'a str {
    tree.children(values)
        .filter(|child| tree.is(*child, "function_definition"))
        .find_map(|child| tree.field(child, Field::Parameters))
        .map_or("()", |parameters| tree.text(parameters))
}

fn symbol(
    tree: &Tree,
    node: NodeId,
    name: &str,
    kind: SymbolKind,
    signature: &str,
) -> Option<Symbol> {
    if name.is_empty() {
        return None;
    }
    let signature = if signature.is_empty() {
        format!("fn {name}")
    } else {
        signature.to_owned()
    };
    Some(
        Symbol::at(tree, node, name, kind)
            .signed(&signature)
            .public(conventionally_public(name)),
    )
}
