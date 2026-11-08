//! The parts of a refusal that the validation and the dispatch share: the value a call gave, the
//! form a parameter accepts, and the list of the tools.

use app::surface::{Param, Shape, TOOLS};
use prompts::messages::{
    ACCEPTS_BOOL, ACCEPTS_CHOICE, ACCEPTS_INTEGER, ACCEPTS_INTEGER_FROM, ACCEPTS_NUMBER,
    ACCEPTS_SCHEMA, ACCEPTS_TEXT, ACCEPTS_TEXT_OR_TEXTS, ACCEPTS_TEXTS, LISTED_AND, LISTED_OR,
    UNKNOWN_TOOL,
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

// The form that `param` accepts.
pub(crate) fn accepted(param: &Param) -> String {
    phrase(&param.shape)
}

// The refusal of a call to a tool the server does not have.
pub(crate) fn unknown_tool() -> String {
    let names: Vec<&str> = TOOLS.iter().map(|tool| tool.name).collect();
    UNKNOWN_TOOL.render(&listed(&names, &LISTED_AND))
}

// The names joined by commas, with the last one joined by `last`.
pub(crate) fn listed(names: &[&str], last: &Template<(Text, Text)>) -> String {
    match names.split_last() {
        Some((only, [])) => (*only).to_owned(),
        Some((final_name, rest)) => last.render((final_name, &rest.join(", "))),
        None => String::new(),
    }
}

// The form of a choice among `names`: a choice of one is the name alone.
fn choice(names: &[&str]) -> String {
    match names {
        [only] => (*only).to_owned(),
        _ => ACCEPTS_CHOICE.render(&listed(names, &LISTED_OR)),
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
        Shape::Literal(names) | Shape::Enum { names, .. } => choice(names),
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
    match shape {
        Shape::Array {
            items: Shape::Text, ..
        } => ACCEPTS_TEXTS.to_owned(),
        Shape::AnyOf(
            [
                Shape::Text,
                Shape::Array {
                    items: Shape::Text,
                    min_items: Some(minimum),
                    max_items: Some(maximum),
                },
            ],
        ) => ACCEPTS_TEXT_OR_TEXTS.render((*minimum, *maximum)),
        _ => ACCEPTS_SCHEMA.to_owned(),
    }
}
