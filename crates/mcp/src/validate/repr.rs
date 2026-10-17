//! How a refusal shows an input: the Python repr of the JSON value, cut to its first 25 and last 24
//! bytes past 50, and the name of its Python type.

use serde_json::Value as Json;
use text::CodePointString;

const SHOWN_BYTES: usize = 50;
const HEAD_BYTES: usize = 25;
const TAIL_BYTES: usize = 24;

// The repr of `input`, cut at character boundaries when it runs past 50 bytes.
pub(super) fn shown(input: &Json) -> String {
    let text = repr(input);
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

pub(super) fn repr(input: &Json) -> String {
    match input {
        Json::Null => "None".to_owned(),
        Json::Bool(true) => "True".to_owned(),
        Json::Bool(false) => "False".to_owned(),
        Json::Number(number) => number_repr(number),
        Json::String(text) => quoted(text),
        Json::Array(items) => {
            let items: Vec<String> = items.iter().map(repr).collect();
            format!("[{}]", items.join(", "))
        }
        Json::Object(entries) => {
            let entries: Vec<String> = entries
                .iter()
                .map(|(key, value)| format!("{}: {}", quoted(key), repr(value)))
                .collect();
            format!("{{{}}}", entries.join(", "))
        }
    }
}

pub(super) fn quoted(text: &str) -> String {
    CodePointString::from(text).quoted()
}

// An integer as its digits, and any other number as the shortest float text.
fn number_repr(number: &serde_json::Number) -> String {
    if number.is_f64() {
        return json::float_text(number.as_f64().unwrap_or(f64::NAN));
    }
    number.to_string()
}
