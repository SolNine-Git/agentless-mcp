//! The `PostToolUse` and Stop payloads of erosion-scan, read in one step into typed values.

use std::path::PathBuf;

use json::JsonValue;

use crate::halt::Halt;
use crate::payload::{boolean, document, object, string};
use crate::text::lossy;

const POST_TOOL_USE: &str = "PostToolUse";
const STOP: &str = "Stop";
const SUBAGENT_STOP: &str = "SubagentStop";

// What the event asks for: the scan of one shell command, or of every marked root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Shell(String),
    Stop { active: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Payload {
    pub(crate) event: &'static str,
    pub(crate) session: String,
    pub(crate) cwd: PathBuf,
    pub(crate) action: Action,
}

// The fields in a fixed order, so a payload with two problems always reports the same one.
pub(crate) fn parse(raw: &[u8]) -> Result<Payload, Halt> {
    let document = document(raw)?;
    let JsonValue::Object(fields) = &document else {
        return Err(Halt::malformed("the payload is not a JSON object"));
    };
    let name = string(fields, "hook_event_name", "payload")?;
    let event = match name.as_str() {
        Some(POST_TOOL_USE) => POST_TOOL_USE,
        Some(STOP) => STOP,
        Some(SUBAGENT_STOP) => SUBAGENT_STOP,
        _ => {
            let shown = lossy(name);
            return Err(Halt::skipped(format!("not a scanned event: {shown}")));
        }
    };
    let session = owned(
        string(fields, "session_id", "payload")?,
        "payload.session_id",
    )?;
    let cwd = owned(string(fields, "cwd", "payload")?, "payload.cwd")?;
    let action = if event == POST_TOOL_USE {
        let input = object(fields, "tool_input", "payload")?;
        let command = string(input, "command", "tool_input")?;
        Action::Shell(owned(command, "tool_input.command")?)
    } else {
        let active = boolean(fields, "stop_hook_active", "payload")?;
        Action::Stop { active }
    };
    Ok(Payload {
        event,
        session,
        cwd: PathBuf::from(cwd),
        action,
    })
}

fn owned(value: &text::CodePointString, place: &str) -> Result<String, Halt> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Halt::malformed(format!("{place} holds a lone surrogate")))
}

#[cfg(test)]
mod tests;
