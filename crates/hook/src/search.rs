//! What the hooks share from the retired search gate: the payload read, the gate log, and the
//! shell parser that finds a single-file read.

pub(crate) mod log;
pub(crate) mod shell;

use std::io::Read;

use json::{JsonErrorKind, JsonValue, get};
use text::CodePointString;

use crate::payload::Fields;

// An error that the hooks answer as the reference's blanket suppress: nothing is written.
#[derive(Debug)]
pub(crate) struct Suppressed;

// The payload on stdin: a JSON value, or text that is not one JSON document.
pub(crate) enum Parsed {
    Value(JsonValue),
    Malformed,
}

// An unreadable or undecodable stdin, and a document past the depth or the digit limit, raise
// another error than a decode error in the reference: they are suppressed.
pub(crate) fn parsed(stdin: &mut dyn Read) -> Result<Parsed, Suppressed> {
    let mut raw = Vec::new();
    stdin.read_to_end(&mut raw).map_err(|_| Suppressed)?;
    let text = io::decode_strict(raw).map_err(|_| Suppressed)?;
    let text = if text.is_empty() { "{}" } else { text.as_str() };
    match json::parse(text) {
        Ok(value) => Ok(Parsed::Value(value)),
        Err(error) if past_a_limit(error.kind) => Err(Suppressed),
        Err(_) => Ok(Parsed::Malformed),
    }
}

fn past_a_limit(kind: JsonErrorKind) -> bool {
    matches!(
        kind,
        JsonErrorKind::ObjectTooDeep
            | JsonErrorKind::ArrayTooDeep
            | JsonErrorKind::IntTooLong { .. }
    )
}

// The field `key` when it holds a string that is not empty.
pub(crate) fn text_field<'a>(fields: &'a Fields, key: &str) -> Option<&'a CodePointString> {
    match get(fields, key) {
        Some(JsonValue::Str(value)) if value.code_points().next().is_some() => Some(value),
        _ => None,
    }
}

pub(crate) fn object_field<'a>(fields: &'a Fields, key: &str) -> Option<&'a Fields> {
    match get(fields, key) {
        Some(JsonValue::Object(value)) => Some(value),
        _ => None,
    }
}
