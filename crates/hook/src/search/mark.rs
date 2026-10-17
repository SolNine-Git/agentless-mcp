//! gate-mark: after a call that shows where code lives, the session's marker; for every call, a
//! log line that names the operation.

use std::borrow::Cow;
use std::io::Read;
use std::panic::{self, AssertUnwindSafe};

use app::EnvConfig;
use app::surface::{Calls, TOOLS};
use json::JsonValue;
use kernel::SystemClock;
use text::CodePointString;

use super::log::{self, Entry, key};
use super::{
    Gate, Parsed, Suppressed, as_text, marker, object_field, parsed, text_field, unhashable,
};
use crate::payload::{Fields, get};

// The prefix that Claude Code puts on the tools of the server it names "agentless".
const TOOL_PREFIX: &str = "mcp__agentless__";
// The v1 tools that localize. A session that a v1 server served still unlocks with them.
const V1_LOCALIZING: [&str; 6] = [
    "repo_map",
    "get_symbols_overview",
    "expand_symbols",
    "find_symbol",
    "explain_symbol",
    "read_slice",
];
const STRUCTURAL_CALL: &str = "structural_call";
const CALL_IGNORED: &str = "agentless_call_ignored";
const NON_LOCALIZING: &str = "non_localizing_operation";

/// Records the `PostToolUse` payload on `stdin`: the session's marker after a localizing call, and
/// the log line.
pub fn gate_mark(env: &EnvConfig, stdin: &mut dyn Read) {
    let clock = SystemClock;
    mark_with(&Gate::of(env, &clock), stdin);
}

// Fail open: a failure costs one early deny, while a failed hook would fail a call that ran.
pub(crate) fn mark_with(gate: &Gate<'_>, stdin: &mut dyn Read) {
    drop(panic::catch_unwind(AssertUnwindSafe(|| {
        marked(gate, stdin)
    })));
}

// A payload that is not one JSON document, or that has no session id, is left alone. A payload
// that is not an object raises in the reference.
fn marked(gate: &Gate<'_>, stdin: &mut dyn Read) -> Result<(), Suppressed> {
    let Parsed::Value(document) = parsed(stdin)? else {
        return Ok(());
    };
    let JsonValue::Object(fields) = &document else {
        return Err(Suppressed);
    };
    let Some(session) = text_field(fields, "session_id") else {
        return Ok(());
    };
    let call = Call {
        session,
        tool: get(fields, "tool_name"),
        operation: operation(fields),
    };
    if unhashable(call.tool) {
        return Err(Suppressed);
    }
    let Some(reason) = unlock_reason(as_text(call.tool), as_text(call.operation)) else {
        return log::append(gate, &call.entry(gate, CALL_IGNORED, NON_LOCALIZING));
    };
    marker::write(gate, session, &reason, &log::value(call.tool))?;
    log::append(gate, &call.entry(gate, STRUCTURAL_CALL, &reason))
}

// The fields of one call that its log line names.
struct Call<'a> {
    session: &'a CodePointString,
    tool: Option<&'a JsonValue>,
    // The grouped tool's operation argument when it is a string.
    operation: Option<&'a JsonValue>,
}

impl Call<'_> {
    fn entry(&self, gate: &Gate<'_>, event: &str, reason: &str) -> Entry<'_> {
        vec![
            (key::TS, log::now(gate.clock)),
            (
                key::SESSION_ID,
                Cow::Owned(JsonValue::Str(self.session.clone())),
            ),
            (key::EVENT, log::text(event)),
            (key::TOOL, log::value(self.tool)),
            (key::OPERATION, log::value(self.operation)),
            (key::REASON, log::text(reason)),
        ]
    }
}

fn operation(fields: &Fields) -> Option<&JsonValue> {
    let operation = object_field(fields, "tool_input").and_then(|input| get(input, "operation"));
    operation.filter(|value| matches!(value, JsonValue::Str(_)))
}

// The localizing call that `tool` and `operation` name: a v1 tool by its name, a v2 tool by the
// surface table, as "tool" or "tool.operation". None for every other call.
pub(crate) fn unlock_reason(tool: Option<&str>, operation: Option<&str>) -> Option<String> {
    let name = tool?.strip_prefix(TOOL_PREFIX)?;
    if V1_LOCALIZING.contains(&name) {
        return Some(name.to_owned());
    }
    let tool = TOOLS.iter().find(|tool| tool.name == name)?;
    if !tool.localizes(operation) {
        return None;
    }
    match (&tool.calls, operation) {
        (Calls::Operations(_), Some(operation)) => Some(format!("{name}.{operation}")),
        _ => Some(name.to_owned()),
    }
}
