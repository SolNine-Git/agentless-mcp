//! The parts of a refusal that the validation, the operation checks and the dispatch share: the
//! value a call gave, the form a parameter accepts, and the tool that has an operation.

use app::surface::{Calls, Operation, Param, Shape, TOOLS, Tool};
use prompts::messages::{
    ACCEPTS_BOOL, ACCEPTS_CHOICE, ACCEPTS_INTEGER, ACCEPTS_INTEGER_FROM, ACCEPTS_LINE_RANGES,
    ACCEPTS_NUMBER, ACCEPTS_SCHEMA, ACCEPTS_TEXT, ACCEPTS_TEXT_OR_TEXTS, ACCEPTS_TEXTS, LISTED_AND,
    LISTED_OR, OPERATION_IN_TOOL, OPERATION_TAKES, OPERATION_TAKES_AND_REQUIRES, UNKNOWN_TOOL,
};
use prompts::{Template, Text};
use serde_json::Value as Json;

#[cfg(test)]
mod tests;

const SHOWN_BYTES: usize = 50;
const HEAD_BYTES: usize = 25;
const TAIL_BYTES: usize = 24;

// The compact JSON text of `input`, cut at character boundaries when it runs past 50 bytes.
pub(crate) fn shown(input: &Json) -> String {
    let text = input.to_string();
    if text.len() <= SHOWN_BYTES {
        return text;
    }
    let head = text.floor_char_boundary(HEAD_BYTES);
    let tail = text.ceil_char_boundary(text.len() - TAIL_BYTES);
    format!("{}...{}", &text[..head], &text[tail..])
}

// The form `param` of `tool` accepts: the operation names for the parameter that selects one.
pub(crate) fn accepted(tool: &Tool, param: &Param) -> String {
    match tool.calls {
        Calls::Operations(table) if param.name == "operation" => {
            let names: Vec<&str> = table.iter().map(|operation| operation.name).collect();
            ACCEPTS_CHOICE.render(&listed(&names, &LISTED_OR))
        }
        _ => phrase(&param.shape),
    }
}

// The form the parameter `name` of `tool` accepts; a name the tool lacks points at the schema.
pub(crate) fn accepted_by_name(tool: &Tool, name: &str) -> String {
    tool.params
        .iter()
        .find(|param| param.name == name)
        .map_or_else(|| ACCEPTS_SCHEMA.to_owned(), |param| accepted(tool, param))
}

// The sentence that lists what `operation` takes and, when it has any, what it requires.
pub(crate) fn operation_takes(operation: &Operation) -> String {
    let accepted = listed(operation.accepted, &LISTED_AND);
    if operation.required.is_empty() {
        OPERATION_TAKES.render((operation.name, &accepted))
    } else {
        let required = listed(operation.required, &LISTED_AND);
        OPERATION_TAKES_AND_REQUIRES.render((operation.name, &accepted, &required))
    }
}

// The sentence that names the tool, other than `except`, whose operation table holds `name`.
pub(crate) fn operation_owner(name: &str, except: &str) -> Option<String> {
    TOOLS
        .iter()
        .find(|tool| tool.name != except && tool.operation(name).is_some())
        .map(|tool| OPERATION_IN_TOOL.render((tool.name, name)))
}

// The refusal of a call to a tool the server does not have.
pub(crate) fn unknown_tool(name: &str) -> String {
    let names: Vec<&str> = TOOLS.iter().map(|tool| tool.name).collect();
    let refusal = UNKNOWN_TOOL.render(&listed(&names, &LISTED_AND));
    match operation_owner(name, "") {
        Some(owner) => format!("{refusal} {owner}"),
        None => refusal,
    }
}

// The names joined by commas, with the last one joined by `last`.
pub(crate) fn listed(names: &[&str], last: &Template<(Text, Text)>) -> String {
    match names.split_last() {
        Some((only, [])) => (*only).to_owned(),
        Some((final_name, rest)) => last.render((final_name, &rest.join(", "))),
        None => String::new(),
    }
}

fn phrase(shape: &Shape) -> String {
    match *shape {
        Shape::Text => ACCEPTS_TEXT.to_owned(),
        Shape::Bool => ACCEPTS_BOOL.to_owned(),
        Shape::Int { minimum, maximum } => integer(minimum, maximum),
        Shape::Number {
            exclusive_minimum,
            maximum,
        } => ACCEPTS_NUMBER.render((exclusive_minimum, &maximum.to_string())),
        Shape::Literal(names) | Shape::Enum { names, .. } => {
            ACCEPTS_CHOICE.render(&listed(names, &LISTED_OR))
        }
        Shape::Array { .. } | Shape::AnyOf(_) => compound(shape),
    }
}

fn integer(minimum: usize, maximum: Option<usize>) -> String {
    match maximum {
        Some(maximum) => ACCEPTS_INTEGER.render((minimum, maximum)),
        None => ACCEPTS_INTEGER_FROM.render(minimum),
    }
}

// A list or a choice of shapes that a refusal names in words, else the pointer to its schema.
fn compound(shape: &Shape) -> String {
    let text = match shape {
        Shape::Array {
            items: Shape::Text, ..
        } => ACCEPTS_TEXTS,
        Shape::AnyOf(
            [
                Shape::Text,
                Shape::Array {
                    items: Shape::Text, ..
                },
            ],
        ) => ACCEPTS_TEXT_OR_TEXTS,
        Shape::AnyOf([pair, Shape::Array { items, .. }])
            if is_line_pair(pair) && is_line_pair(items) =>
        {
            ACCEPTS_LINE_RANGES
        }
        _ => ACCEPTS_SCHEMA,
    };
    text.to_owned()
}

// Two integers of 1 or more with no upper bound: a [start, end] pair of line numbers.
fn is_line_pair(shape: &Shape) -> bool {
    matches!(
        shape,
        Shape::Array {
            items: Shape::Int {
                minimum: 1,
                maximum: None
            },
            min_items: Some(2),
            max_items: Some(2),
        }
    )
}
