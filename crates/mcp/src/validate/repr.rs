//! How a refusal shows an input: the Python repr of the JSON value, cut to its first 25 and last 24
//! bytes past 50, and the name of its Python type.

use json::JsonValue;
use serde_json::Value as Json;
use text::CodePointString;

const SHOWN_BYTES: usize = 50;
const HEAD_BYTES: usize = 25;
const TAIL_BYTES: usize = 24;

// The repr of `input`, cut at character boundaries when it runs past 50 bytes.
pub(super) fn shown(input: &Json) -> String {
    let text = json::repr_text(&parsed(input));
    if text.len() <= SHOWN_BYTES {
        return text;
    }
    let head = text.floor_char_boundary(HEAD_BYTES);
    let tail = text.ceil_char_boundary(text.len() - TAIL_BYTES);
    format!("{}...{}", &text[..head], &text[tail..])
}

pub(super) fn type_name(input: &Json) -> &'static str {
    match input {
        Json::Null => "NoneType",
        Json::Bool(_) => "bool",
        Json::Number(number) if number.is_f64() => "float",
        Json::Number(_) => "int",
        Json::String(_) => "str",
        Json::Array(_) => "list",
        Json::Object(_) => "dict",
    }
}

// The input as a parsed value: an f64 number is a float, and any other number keeps its digits.
// serde_json reads at most 128 levels, so the recursion stays shallow.
fn parsed(input: &Json) -> JsonValue {
    match input {
        Json::Null => JsonValue::Null,
        Json::Bool(value) => JsonValue::Bool(*value),
        Json::Number(number) if number.is_f64() => {
            JsonValue::Float(number.as_f64().unwrap_or(f64::NAN))
        }
        Json::Number(number) => JsonValue::Int(number.to_string()),
        Json::String(text) => JsonValue::Str(text.as_str().into()),
        Json::Array(items) => JsonValue::Array(items.iter().map(parsed).collect()),
        Json::Object(entries) => JsonValue::Object(
            entries
                .iter()
                .map(|(key, value)| (key.as_str().into(), parsed(value)))
                .collect(),
        ),
    }
}

pub(super) fn quoted(text: &str) -> String {
    CodePointString::from(text).quoted()
}
