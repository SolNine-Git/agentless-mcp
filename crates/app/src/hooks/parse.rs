//! The one parse of a settings `hooks` value: each handler with its event and its group's
//! matcher, or the place where the value has another shape than Claude Code reads.

use json::{JsonValue, get};

/// One handler of a settings `hooks` value.
#[derive(Clone, Debug, PartialEq)]
pub struct Handler {
    /// The event key it sits under, such as `PreToolUse`.
    pub event: String,
    /// Its group's matcher, or `None` when the group has none.
    pub matcher: Option<String>,
    /// Its `type`, such as `command`, or `None` when it has none.
    pub kind: Option<String>,
    /// Its `command`, or `None` for a handler that has none.
    pub command: Option<String>,
    /// Its `timeout`, in seconds, or `None` when it has none.
    pub timeout: Option<f64>,
}

/// Where a settings `hooks` value has another shape than Claude Code reads, and what it expects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HooksShape {
    /// The place, such as `PreToolUse[0].hooks[1].timeout`, or empty for the value itself.
    pub at: String,
    /// What the place must hold.
    pub expected: Expected,
}

/// The JSON type that a place of the `hooks` value must hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expected {
    /// An object.
    Object,
    /// An array.
    Array,
    /// A string of Unicode text.
    Text,
    /// A number.
    Number,
}

/// Reads `hooks`, the value of a settings file's `hooks` key, into its handlers in file order.
///
/// # Errors
///
/// Returns the first place whose value has another shape: the value, an event, a group, a group's
/// `hooks`, a handler, or a handler's `type`, `command` or `timeout`.
pub fn handlers(hooks: &JsonValue) -> Result<Vec<Handler>, HooksShape> {
    let events = object(hooks, String::new())?;
    let mut found = Vec::new();
    for (event, groups) in events {
        let event = (event.as_str()).ok_or_else(|| shape(String::new(), Expected::Text))?;
        let event = event.to_owned();
        for (at, group) in array(groups, event.clone())?.iter().enumerate() {
            found.extend(group_handlers(&event, group, &format!("{event}[{at}]"))?);
        }
    }
    Ok(found)
}

fn group_handlers(event: &str, group: &JsonValue, at: &str) -> Result<Vec<Handler>, HooksShape> {
    let fields = object(group, at.to_owned())?;
    let matcher = optional(fields, "matcher", at, text)?;
    let Some(listed) = get(fields, "hooks") else {
        return Ok(Vec::new());
    };
    let place = format!("{at}.hooks");
    let listed = array(listed, place.clone())?;
    let handlers = listed.iter().enumerate().map(|(index, handler)| {
        let fields = object(handler, format!("{place}[{index}]"))?;
        let at = format!("{place}[{index}]");
        Ok(Handler {
            event: event.to_owned(),
            matcher: matcher.clone(),
            kind: optional(fields, "type", &at, text)?,
            command: optional(fields, "command", &at, text)?,
            timeout: optional(fields, "timeout", &at, number)?,
        })
    });
    handlers.collect()
}

fn optional<T>(
    fields: &[(text::CodePointString, JsonValue)],
    key: &str,
    at: &str,
    read: fn(&JsonValue, String) -> Result<T, HooksShape>,
) -> Result<Option<T>, HooksShape> {
    get(fields, key)
        .map(|value| read(value, format!("{at}.{key}")))
        .transpose()
}

fn object(
    value: &JsonValue,
    at: String,
) -> Result<&[(text::CodePointString, JsonValue)], HooksShape> {
    match value {
        JsonValue::Object(fields) => Ok(fields),
        _ => Err(shape(at, Expected::Object)),
    }
}

fn array(value: &JsonValue, at: String) -> Result<&[JsonValue], HooksShape> {
    match value {
        JsonValue::Array(items) => Ok(items),
        _ => Err(shape(at, Expected::Array)),
    }
}

fn text(value: &JsonValue, at: String) -> Result<String, HooksShape> {
    match value {
        JsonValue::Str(text) => text
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| shape(at, Expected::Text)),
        _ => Err(shape(at, Expected::Text)),
    }
}

fn number(value: &JsonValue, at: String) -> Result<f64, HooksShape> {
    let read = match value {
        JsonValue::Int(digits) => digits.parse::<f64>().ok(),
        JsonValue::Float(number) => Some(*number),
        _ => None,
    };
    read.ok_or_else(|| shape(at, Expected::Number))
}

fn shape(at: String, expected: Expected) -> HooksShape {
    HooksShape { at, expected }
}
