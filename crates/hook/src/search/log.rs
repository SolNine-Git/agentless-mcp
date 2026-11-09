//! The gate log: one JSON line per hook run, appended to the file that `AGENTLESS_GATE_LOG` names.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::path::Path;

use json::JsonValue;
use kernel::Clock;

use super::Suppressed;
use crate::record;

pub(crate) const LOG_ENV: &str = "AGENTLESS_GATE_LOG";

// The fields of a log line in the order the line writes them. A payload value is borrowed: it
// can nest 9998 levels deep, and a clone of it would recurse.
pub(crate) type Entry<'a> = Vec<(&'static str, Cow<'a, JsonValue>)>;

pub(crate) mod key {
    pub(crate) const TS: &str = "ts";
    pub(crate) const SESSION_ID: &str = "session_id";
    pub(crate) const EVENT: &str = "event";
    pub(crate) const REASON: &str = "reason";
}

pub(crate) fn now(clock: &dyn Clock) -> Cow<'static, JsonValue> {
    Cow::Owned(JsonValue::Float(record::seconds(clock.wall())))
}

pub(crate) fn text(value: &str) -> Cow<'static, JsonValue> {
    Cow::Owned(JsonValue::Str(value.into()))
}

// An unset or empty variable writes nothing. A failed append is suppressed, deny included.
pub(crate) fn append(log: Option<&OsStr>, entry: &Entry<'_>) -> Result<(), Suppressed> {
    let Some(target) = log.filter(|target| !target.is_empty()) else {
        return Ok(());
    };
    io::append_line(Path::new(target), line(entry).as_bytes()).map_err(|_| Suppressed)
}

// The keys are plain ASCII names, so each is its own JSON text between quotes.
fn line(entry: &Entry<'_>) -> String {
    let fields: Vec<String> = entry
        .iter()
        .map(|(name, item)| format!("\"{name}\": {}", json::value_text(item)))
        .collect();
    format!("{{{}}}\n", fields.join(", "))
}
