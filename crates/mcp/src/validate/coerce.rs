//! The lax coercion of one JSON value to one parameter shape, or `None` for a value that does not
//! fit.

use std::cmp::Ordering;

use app::IntArg;
use app::surface::Shape;
use serde_json::Value as Json;

use crate::arguments::Value;

// The longest integer text that converts.
const MAX_INT_TEXT: usize = 4300;
// The floats strictly between these two hold an i64.
const I64_FLOOR: f64 = -9_223_372_036_854_775_808.0;
const I64_CEILING: f64 = 9_223_372_036_854_775_808.0;

pub(super) fn coerce(shape: &Shape, input: &Json) -> Option<Value> {
    match *shape {
        Shape::Text => text(input),
        Shape::Bool => boolean(input).map(Value::Bool),
        Shape::Int { minimum, maximum } => int_bounds(integer(input)?, minimum, maximum),
        Shape::Number {
            exclusive_minimum,
            maximum,
        } => number(input, exclusive_minimum, maximum),
        Shape::Literal(names) | Shape::Enum { names, .. } => choice(input, names),
        Shape::Array {
            items,
            min_items,
            max_items,
        } => list(input, items, (min_items, max_items)),
        Shape::AnyOf(shapes) => shapes.iter().find_map(|shape| coerce(shape, input)),
    }
}

fn text(input: &Json) -> Option<Value> {
    match input {
        Json::String(text) => Some(Value::Text(text.clone())),
        _ => None,
    }
}

fn boolean(input: &Json) -> Option<bool> {
    match input {
        Json::Bool(flag) => Some(*flag),
        Json::Number(number) => number_flag(number),
        Json::String(word) => word_flag(word),
        _ => None,
    }
}

// An integer reads as a flag when it is 0 or 1, and a float when it is 0.0 or 1.0.
fn number_flag(number: &serde_json::Number) -> Option<bool> {
    if number.is_f64() {
        let value = number.as_f64().unwrap_or(f64::NAN);
        return [0.0, 1.0]
            .iter()
            .position(|flag| equals(value, *flag))
            .map(|position| position == 1);
    }
    match number.as_u64() {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    }
}

fn word_flag(word: &str) -> Option<bool> {
    match word.to_ascii_lowercase().as_str() {
        "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
        "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
        _ => None,
    }
}

// The integer of a bool, a number or a text, or `None` for an input that holds none.
fn integer(input: &Json) -> Option<IntArg> {
    match input {
        Json::Bool(flag) => Some(IntArg::from_usize(usize::from(*flag))),
        Json::Number(number) if number.is_f64() => float_int(number.as_f64().unwrap_or(f64::NAN)),
        Json::Number(number) => IntArg::parse(&number.to_string()),
        Json::String(text) => text_int(text),
        _ => None,
    }
}

fn equals(left: f64, right: f64) -> bool {
    left.partial_cmp(&right) == Some(Ordering::Equal)
}

fn float_int(value: f64) -> Option<IntArg> {
    let whole = equals(value.fract(), 0.0) && I64_FLOOR < value && value < I64_CEILING;
    whole.then(|| IntArg::parse(&format!("{value:.0}")))?
}

// Whitespace around, a sign, digits with single underscores between them, and a `.0…` tail.
fn text_int(text: &str) -> Option<IntArg> {
    let trimmed = text.trim();
    if trimmed.len() > MAX_INT_TEXT {
        return None;
    }
    let integral = without_decimal_zeros(trimmed);
    if !is_grouped_int(integral) {
        return None;
    }
    IntArg::parse(integral)
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

fn int_bounds(value: IntArg, minimum: usize, maximum: Option<usize>) -> Option<Value> {
    let number = value.value().filter(|number| *number >= minimum)?;
    if maximum.is_some_and(|maximum| number > maximum) {
        return None;
    }
    Some(Value::Int(value))
}

fn number(input: &Json, exclusive_minimum: usize, maximum: f64) -> Option<Value> {
    let value = match input {
        Json::Bool(flag) => f64::from(u8::from(*flag)),
        Json::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Json::String(text) => text_float(text)?,
        _ => return None,
    };
    let floor = u32::try_from(exclusive_minimum).map_or(f64::INFINITY, f64::from);
    let within = matches!(
        value.partial_cmp(&maximum),
        Some(Ordering::Less | Ordering::Equal)
    ) && matches!(value.partial_cmp(&floor), Some(Ordering::Greater));
    within.then_some(Value::Float(value))
}

// The trimmed text, else the untrimmed text without its single inner underscores, so whitespace
// beside an underscore fails.
fn text_float(text: &str) -> Option<f64> {
    text.trim()
        .parse()
        .ok()
        .or_else(|| without_underscores(text)?.parse().ok())
}

// The text with its underscores dropped, or None when one is at either end or two touch.
fn without_underscores(text: &str) -> Option<String> {
    let misplaced = text.starts_with('_') || text.ends_with('_') || text.contains("__");
    (!misplaced).then(|| text.replace('_', ""))
}

fn choice(input: &Json, names: &[&'static str]) -> Option<Value> {
    let Json::String(given) = input else {
        return None;
    };
    names
        .iter()
        .find(|name| **name == given.as_str())
        .map(|name| Value::Text((*name).to_owned()))
}

fn list(
    input: &Json,
    items: &Shape,
    (min_items, max_items): (Option<usize>, Option<usize>),
) -> Option<Value> {
    let Json::Array(values) = input else {
        return None;
    };
    if max_items.is_some_and(|most| values.len() > most)
        || min_items.is_some_and(|least| values.len() < least)
    {
        return None;
    }
    let found: Option<Vec<Value>> = values.iter().map(|value| coerce(items, value)).collect();
    found.map(Value::List)
}
