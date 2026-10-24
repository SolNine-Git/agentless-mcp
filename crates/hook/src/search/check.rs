//! gate-check: the decision on one `PreToolUse` payload, its log line, and the deny message.

use std::borrow::Cow;
use std::io::{Read, Write};
use std::panic::{self, AssertUnwindSafe};

use app::EnvConfig;
use json::{JsonValue, get};
use kernel::SystemClock;
use text::CodePointString;

use super::log::{self, Entry, key};
use super::stat::{self, Base};
use super::{
    Gate, Parsed, Suppressed, as_text, marker, object_field, parsed, shell, text_field, unhashable,
};
use crate::payload::Fields;

const ALLOW: u8 = 0;
const DENY: u8 = 2;
// The reference interpreter exits 120 when it cannot flush stderr at exit.
const UNFLUSHED: u8 = 120;

const ALLOWED: &str = "native_search_allowed";
const DENIED: &str = "denied";

const MALFORMED_PAYLOAD: &str = "malformed_payload";
const NO_SESSION_ID: &str = "no_session_id";
const NOT_A_SEARCH_TOOL: &str = "not_a_search_tool";
const ALREADY_UNLOCKED: &str = "already_unlocked";
const EXACT_FILE_SCOPE: &str = "exact_file_scope";
const NOT_A_TREE_SEARCH: &str = "not_a_tree_search";
const MARKER_STORAGE_UNAVAILABLE: &str = "marker_storage_unavailable";
const GLOB_BEFORE_STRUCTURE: &str = "glob_before_structure";
const BROAD_GREP_BEFORE_STRUCTURE: &str = "broad_grep_before_structure";
const SHELL_SEARCH_BEFORE_STRUCTURE: &str = "shell_search_before_structure";

const GREP: &str = "Grep";
const GLOB: &str = "Glob";
const BASH: &str = "Bash";
// The unified-exec tool of Codex, which carries the same command string as Bash.
const EXEC_COMMAND: &str = "exec_command";

const DENY_MESSAGE: &str = "\
Structural pass first: this session has not localized with agentless yet.

Call mcp__agentless__orient with operation=\"map\" and focus seeds from the task \
(file paths or symbol names). Escalate with mcp__agentless__symbols \
operation=\"expand\" on the stable ids it returns.

Broad text search unlocks after that call, whether it runs as Grep, Glob, or \
grep/rg/find inside Bash. A Grep scoped to one existing file is already \
allowed, and so is a shell command that filters another command's output. Use \
broad search afterwards for string literals, error messages, config keys, and \
test names.
";

/// Decides the `PreToolUse` payload on `stdin`, writes the deny message to `stderr`, and returns
/// the exit code: 2 to deny the search, 120 when the deny message cannot be written, else 0.
pub fn gate_check(env: &EnvConfig, stdin: &mut dyn Read, stderr: &mut dyn Write) -> u8 {
    let clock = SystemClock;
    check_with(&Gate::of(env, &clock), stdin, stderr)
}

// Fail open: a panic or a suppressed error allows the call.
pub(crate) fn check_with(gate: &Gate<'_>, stdin: &mut dyn Read, stderr: &mut dyn Write) -> u8 {
    match panic::catch_unwind(AssertUnwindSafe(|| logged(gate, stdin))) {
        Ok(Ok(Verdict { denied: true, .. })) => deny(stderr),
        _ => ALLOW,
    }
}

// A deny whose message cannot be written does not block the call.
fn deny(stderr: &mut dyn Write) -> u8 {
    match stderr
        .write_all(DENY_MESSAGE.as_bytes())
        .and_then(|()| stderr.flush())
    {
        Ok(()) => DENY,
        Err(_) => UNFLUSHED,
    }
}

#[derive(Clone, Copy, Debug)]
struct Verdict {
    denied: bool,
    reason: &'static str,
}

const fn allowed(reason: &'static str) -> Verdict {
    Verdict {
        denied: false,
        reason,
    }
}

const fn denied(reason: &'static str) -> Verdict {
    Verdict {
        denied: true,
        reason,
    }
}

fn logged(gate: &Gate<'_>, stdin: &mut dyn Read) -> Result<Verdict, Suppressed> {
    let document = parsed(stdin)?;
    let ts = log::now(gate.clock);
    let Parsed::Value(JsonValue::Object(fields)) = &document else {
        log::append(gate, &unread_entry(ts))?;
        return Ok(allowed(MALFORMED_PAYLOAD));
    };
    let verdict = decide(gate, fields)?;
    log::append(gate, &entry(ts, fields, verdict))?;
    Ok(verdict)
}

fn unread_entry(ts: Cow<'static, JsonValue>) -> Entry<'static> {
    vec![
        (key::TS, ts),
        (key::SESSION_ID, log::value(None)),
        (key::EVENT, log::text(ALLOWED)),
        (key::REASON, log::text(MALFORMED_PAYLOAD)),
    ]
}

fn entry<'a>(ts: Cow<'static, JsonValue>, fields: &'a Fields, verdict: Verdict) -> Entry<'a> {
    let pattern = object_field(fields, "tool_input").and_then(|input| get(input, "pattern"));
    let event = if verdict.denied { DENIED } else { ALLOWED };
    vec![
        (key::TS, ts),
        (key::SESSION_ID, log::value(get(fields, "session_id"))),
        (key::TOOL, log::value(get(fields, "tool_name"))),
        (key::PATTERN, log::value(pattern)),
        (key::EVENT, log::text(event)),
        (key::REASON, log::text(verdict.reason)),
    ]
}

// A payload with no session id is allowed: no marker can ever unlock that session.
fn decide(gate: &Gate<'_>, fields: &Fields) -> Result<Verdict, Suppressed> {
    let Some(session) = text_field(fields, "session_id") else {
        return Ok(allowed(NO_SESSION_ID));
    };
    let tool = get(fields, "tool_name");
    if unhashable(tool) {
        return Err(Suppressed);
    }
    match as_text(tool) {
        Some(BASH | EXEC_COMMAND) => decide_shell(gate, fields, session),
        Some(name @ (GREP | GLOB)) => decide_native(gate, fields, session, name),
        _ => Ok(allowed(NOT_A_SEARCH_TOOL)),
    }
}

fn decide_native(
    gate: &Gate<'_>,
    fields: &Fields,
    session: &CodePointString,
    tool: &str,
) -> Result<Verdict, Suppressed> {
    let input = object_field(fields, "tool_input");
    if input
        .and_then(|input| text_field(input, "pattern"))
        .is_none()
    {
        return Ok(allowed(MALFORMED_PAYLOAD));
    }
    if marker::exists(gate, session)? {
        return Ok(allowed(ALREADY_UNLOCKED));
    }
    if tool == GREP && exact_file(gate, fields, input)? {
        return Ok(allowed(EXACT_FILE_SCOPE));
    }
    if !marker::storage_ready(gate) {
        return Ok(allowed(MARKER_STORAGE_UNAVAILABLE));
    }
    Ok(denied(if tool == GLOB {
        GLOB_BEFORE_STRUCTURE
    } else {
        BROAD_GREP_BEFORE_STRUCTURE
    }))
}

// A Grep whose path names one existing file has localized its search already.
fn exact_file(
    gate: &Gate<'_>,
    fields: &Fields,
    input: Option<&Fields>,
) -> Result<bool, Suppressed> {
    let Some(path) = input.and_then(|input| text_field(input, "path")) else {
        return Ok(false);
    };
    let found = stat::found(&payload_cwd(gate, fields)?, path)?;
    Ok(found.is_some_and(|metadata| metadata.is_file()))
}

// The marker is tested before the command is read: after the unlock nothing is parsed.
fn decide_shell(
    gate: &Gate<'_>,
    fields: &Fields,
    session: &CodePointString,
) -> Result<Verdict, Suppressed> {
    let input = object_field(fields, "tool_input");
    let Some(command) = input.and_then(|input| text_field(input, "command")) else {
        return Ok(allowed(MALFORMED_PAYLOAD));
    };
    if marker::exists(gate, session)? {
        return Ok(allowed(ALREADY_UNLOCKED));
    }
    if !shell::broad_search(command, &payload_cwd(gate, fields)?)? {
        return Ok(allowed(NOT_A_TREE_SEARCH));
    }
    if !marker::storage_ready(gate) {
        return Ok(allowed(MARKER_STORAGE_UNAVAILABLE));
    }
    Ok(denied(SHELL_SEARCH_BEFORE_STRUCTURE))
}

// The directory a relative operand resolves against: the payload's cwd, else the start-up
// directory. An unreadable start-up directory raises in the reference.
fn payload_cwd<'a>(gate: &Gate<'a>, fields: &'a Fields) -> Result<Base<'a>, Suppressed> {
    match text_field(fields, "cwd") {
        Some(cwd) => Ok(Base::Text(cwd)),
        None => gate.cwd.map(Base::Path).ok_or(Suppressed),
    }
}
