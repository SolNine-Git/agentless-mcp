//! The lax coercion of one JSON value to one parameter shape, with the problems of a value that
//! does not fit.

use std::cmp::Ordering;

use app::IntArg;
use app::surface::Shape;
use serde_json::Value as Json;

use super::repr::quoted;
use crate::arguments::Value;

// The longest integer text that converts.
const MAX_INT_TEXT: usize = 4300;
// The floats strictly between these two hold an i64.
const I64_FLOOR: f64 = -9_223_372_036_854_775_808.0;
const I64_CEILING: f64 = 9_223_372_036_854_775_808.0;

const INT_TYPE: &str = "Input should be a valid integer";
const INT_PARSING: &str = "Input should be a valid integer, unable to parse string as an integer";
const INT_FROM_FLOAT: &str = "Input should be a valid integer, got a number with a fractional part";
const INT_PARSING_SIZE: &str = "Unable to parse input string as an integer, exceeded maximum size";
const FLOAT_TYPE: &str = "Input should be a valid number";
const FLOAT_PARSING: &str = "Input should be a valid number, unable to parse string as a number";
const BOOL_TYPE: &str = "Input should be a valid boolean";
const BOOL_PARSING: &str = "Input should be a valid boolean, unable to interpret input";
const STRING_TYPE: &str = "Input should be a valid string";
const LIST_TYPE: &str = "Input should be a valid list";

// One value that does not fit its shape: where it is, the error type, its message, and the input.
pub(super) struct Problem<'a> {
    pub(super) loc: String,
    pub(super) kind: &'static str,
    pub(super) message: String,
    pub(super) input: &'a Json,
}

pub(super) type Coerced<'a> = Result<Value, Vec<Problem<'a>>>;

pub(super) fn problem<'a>(
    loc: &str,
    kind: &'static str,
    message: impl Into<String>,
    input: &'a Json,
) -> Vec<Problem<'a>> {
    vec![Problem {
        loc: loc.to_owned(),
        kind,
        message: message.into(),
        input,
    }]
}

pub(super) fn coerce<'a>(shape: &Shape, input: &'a Json, loc: &str) -> Coerced<'a> {
    match *shape {
        Shape::Text => text(input, loc),
        Shape::Bool => boolean(input, loc),
        Shape::Int { minimum, maximum } => {
            let value = integer(input).map_err(|kind| int_problem(kind, input, loc))?;
            int_bounds(value, minimum, maximum, input, loc)
        }
        Shape::Number {
            exclusive_minimum,
            maximum,
        } => number(input, loc, exclusive_minimum, maximum),
        Shape::Literal(names) => choice(input, loc, names, "literal_error"),
        Shape::Enum { names, .. } => choice(input, loc, names, "enum"),
        Shape::Array {
            items,
            min_items,
            max_items,
        } => list(input, loc, items, (min_items, max_items)),
        Shape::AnyOf(shapes) => union(input, loc, shapes),
    }
}

fn text<'a>(input: &'a Json, loc: &str) -> Coerced<'a> {
    match input {
        Json::String(text) => Ok(Value::Text(text.clone())),
        _ => Err(problem(loc, "string_type", STRING_TYPE, input)),
    }
}

fn boolean<'a>(input: &'a Json, loc: &str) -> Coerced<'a> {
    let found = match input {
        Json::Bool(flag) => Ok(*flag),
        Json::Number(number) => number_flag(number),
        Json::String(word) => word_flag(word).ok_or(("bool_parsing", BOOL_PARSING)),
        _ => Err(("bool_type", BOOL_TYPE)),
    };
    found
        .map(Value::Bool)
        .map_err(|(kind, message)| problem(loc, kind, message, input))
}

// An integer reads as a flag when it is 0 or 1, and a float when it is 0.0 or 1.0.
fn number_flag(number: &serde_json::Number) -> Result<bool, (&'static str, &'static str)> {
    if number.is_f64() {
        let value = number.as_f64().unwrap_or(f64::NAN);
        return [0.0, 1.0]
            .iter()
            .position(|flag| equals(value, *flag))
            .map(|position| position == 1)
            .ok_or(("bool_type", BOOL_TYPE));
    }
    match number.as_u64() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err(("bool_parsing", BOOL_PARSING)),
    }
}

fn word_flag(word: &str) -> Option<bool> {
    match word.to_ascii_lowercase().as_str() {
        "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
        "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
        _ => None,
    }
}

// The integer of a bool, a number or a text, or the error type of an input that holds none.
fn integer(input: &Json) -> Result<IntArg, &'static str> {
    match input {
        Json::Bool(flag) => Ok(IntArg::from_usize(usize::from(*flag))),
        Json::Number(number) if number.is_f64() => float_int(number.as_f64().unwrap_or(f64::NAN)),
        Json::Number(number) => IntArg::parse(&number.to_string()).ok_or("int_parsing"),
        Json::String(text) => text_int(text),
        _ => Err("int_type"),
    }
}

fn int_problem<'a>(kind: &'static str, input: &'a Json, loc: &str) -> Vec<Problem<'a>> {
    let message = match kind {
        "int_parsing" => INT_PARSING,
        "int_from_float" => INT_FROM_FLOAT,
        "int_parsing_size" => INT_PARSING_SIZE,
        _ => INT_TYPE,
    };
    problem(loc, kind, message, input)
}

fn equals(left: f64, right: f64) -> bool {
    left.partial_cmp(&right) == Some(Ordering::Equal)
}

fn float_int(value: f64) -> Result<IntArg, &'static str> {
    if !equals(value.fract(), 0.0) {
        return Err("int_from_float");
    }
    if !(I64_FLOOR < value && value < I64_CEILING) {
        return Err("int_parsing_size");
    }
    IntArg::parse(&format!("{value:.0}")).ok_or("int_parsing")
}

// Whitespace around, a sign, digits with single underscores between them, and a `.0…` tail.
fn text_int(text: &str) -> Result<IntArg, &'static str> {
    let trimmed = text.trim();
    if trimmed.len() > MAX_INT_TEXT {
        return Err("int_parsing_size");
    }
    let integral = without_decimal_zeros(trimmed);
    if !is_grouped_int(integral) {
        return Err("int_parsing");
    }
    IntArg::parse(integral).ok_or("int_parsing_size")
}

fn without_decimal_zeros(text: &str) -> &str {
    match text.split_once('.') {
        Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|byte| byte == b'0') => head,
        _ => text,
    }
}

fn is_grouped_int(text: &str) -> bool {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    digits
        .split('_')
        .all(|group| !group.is_empty() && group.bytes().all(|byte| byte.is_ascii_digit()))
}

fn int_bounds<'a>(
    value: IntArg,
    minimum: usize,
    maximum: Option<usize>,
    input: &'a Json,
    loc: &str,
) -> Coerced<'a> {
    if value.value().is_none_or(|number| number < minimum) {
        let message = format!("Input should be greater than or equal to {minimum}");
        return Err(problem(loc, "greater_than_equal", message, input));
    }
    if let Some(maximum) = maximum
        && value.value().is_some_and(|number| number > maximum)
    {
        let message = format!("Input should be less than or equal to {maximum}");
        return Err(problem(loc, "less_than_equal", message, input));
    }
    Ok(Value::Int(value))
}

fn number<'a>(input: &'a Json, loc: &str, exclusive_minimum: usize, maximum: f64) -> Coerced<'a> {
    let value = match input {
        Json::Bool(flag) => f64::from(u8::from(*flag)),
        Json::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Json::String(text) => {
            text_float(text).ok_or_else(|| problem(loc, "float_parsing", FLOAT_PARSING, input))?
        }
        _ => return Err(problem(loc, "float_type", FLOAT_TYPE, input)),
    };
    if !matches!(
        value.partial_cmp(&maximum),
        Some(Ordering::Less | Ordering::Equal)
    ) {
        let message = format!("Input should be less than or equal to {maximum}");
        return Err(problem(loc, "less_than_equal", message, input));
    }
    let floor = u32::try_from(exclusive_minimum).map_or(f64::INFINITY, f64::from);
    if !matches!(value.partial_cmp(&floor), Some(Ordering::Greater)) {
        let message = format!("Input should be greater than {exclusive_minimum}");
        return Err(problem(loc, "greater_than", message, input));
    }
    Ok(Value::Float(value))
}

// Whitespace around, and single underscores anywhere but at either end, which the parse drops.
fn text_float(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    let misplaced = trimmed.starts_with('_') || trimmed.ends_with('_') || trimmed.contains("__");
    if misplaced {
        return None;
    }
    trimmed.replace('_', "").parse().ok()
}

fn choice<'a>(
    input: &'a Json,
    loc: &str,
    names: &[&'static str],
    kind: &'static str,
) -> Coerced<'a> {
    if let Json::String(given) = input
        && let Some(name) = names.iter().find(|name| **name == given.as_str())
    {
        return Ok(Value::Text((*name).to_owned()));
    }
    let message = format!("Input should be {}", expected(names));
    Err(problem(loc, kind, message, input))
}

// The quoted names, with "or" before the last one.
fn expected(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| quoted(name)).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

fn list<'a>(
    input: &'a Json,
    loc: &str,
    items: &Shape,
    (min_items, max_items): (Option<usize>, Option<usize>),
) -> Coerced<'a> {
    let Json::Array(values) = input else {
        return Err(problem(loc, "list_type", LIST_TYPE, input));
    };
    if let Some(most) = max_items
        && values.len() > most
    {
        let message = count_message("at most", most, values.len());
        return Err(problem(loc, "too_long", message, input));
    }
    let mut found = Vec::new();
    let mut problems = Vec::new();
    for (index, value) in values.iter().enumerate() {
        match coerce(items, value, &format!("{loc}.{index}")) {
            Ok(item) => found.push(item),
            Err(mut failed) => problems.append(&mut failed),
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    match min_items {
        Some(least) if found.len() < least => {
            let message = count_message("at least", least, found.len());
            Err(problem(loc, "too_short", message, input))
        }
        _ => Ok(Value::List(found)),
    }
}

fn count_message(bound: &str, count: usize, actual: usize) -> String {
    let noun = if count == 1 { "item" } else { "items" };
    format!("List should have {bound} {count} {noun} after validation, not {actual}")
}

// The first shape that takes the input, else the problems of every shape under its own tag.
fn union<'a>(input: &'a Json, loc: &str, shapes: &[Shape]) -> Coerced<'a> {
    let mut problems = Vec::new();
    for shape in shapes {
        match coerce(shape, input, &format!("{loc}.{}", tag(shape))) {
            Ok(value) => return Ok(value),
            Err(mut failed) => problems.append(&mut failed),
        }
    }
    Err(problems)
}

// The name of a shape's Python type, which tags a choice's problems.
pub(super) fn tag(shape: &Shape) -> String {
    match shape {
        Shape::Text => "str".to_owned(),
        Shape::Bool => "bool".to_owned(),
        Shape::Int { .. } => "int".to_owned(),
        Shape::Number { .. } => "float".to_owned(),
        Shape::Array { items, .. } => format!("list[{}]", tag(items)),
        Shape::Literal(_) | Shape::Enum { .. } | Shape::AnyOf(_) => "union".to_owned(),
    }
}
