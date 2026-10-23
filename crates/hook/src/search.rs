//! The search gate: gate-check holds broad text search until the session makes a localizing
//! agentless call, and gate-mark records that call in a marker file named for the session.

mod check;
mod log;
mod mark;
mod marker;
mod shell;
mod stat;

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};

use app::EnvConfig;
use json::{JsonErrorKind, JsonValue, get};
use kernel::Clock;
use text::CodePointString;

use crate::payload::Fields;

pub use check::gate_check;
pub use mark::gate_mark;

// An error that the gate answers as the reference's blanket suppress does: the call is allowed,
// and no log line and no stderr text are written.
#[derive(Debug)]
pub(crate) struct Suppressed;

// What one gate run reads from its process: the marker directory, the log variable, the
// start-up directory, the clock and the process id.
pub(crate) struct Gate<'a> {
    pub(crate) markers: PathBuf,
    pub(crate) log: Option<&'a OsStr>,
    // None when the start-up directory cannot be read.
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) clock: &'a dyn Clock,
    pub(crate) pid: u32,
}

impl<'a> Gate<'a> {
    fn of(env: &'a EnvConfig, clock: &'a dyn Clock) -> Self {
        Self {
            markers: PathBuf::from(marker::MARKER_DIR),
            log: env.var(log::LOG_ENV),
            cwd: env.cwd().ok(),
            clock,
            pid: std::process::id(),
        }
    }
}

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

// An array or an object cannot be a member of the reference's name sets: the test raises.
pub(crate) fn unhashable(value: Option<&JsonValue>) -> bool {
    matches!(value, Some(JsonValue::Array(_) | JsonValue::Object(_)))
}

pub(crate) fn as_text(value: Option<&JsonValue>) -> Option<&str> {
    match value {
        Some(JsonValue::Str(text)) => text.as_str(),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
