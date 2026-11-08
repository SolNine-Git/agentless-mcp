//! What each hook of the trace surface shares: the process facts it reads, the payload's object
//! and context, the panic catch that keeps a failure silent, and its one log line.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Instant;

use app::EnvConfig;
use json::{JsonValue, get};
use kernel::Clock;
use text::CodePointString;

use crate::ledger;
use crate::reply::{self, EMITTED, INTERNAL};
use crate::search::log::{self, Entry, key};
use crate::search::{Parsed, object_field, parsed, text_field};

pub(crate) const MALFORMED_PAYLOAD: &str = "malformed_payload";
pub(crate) const NO_SESSION_ID: &str = "no_session_id";
pub(crate) const NO_REPOSITORY: &str = "no_repository";
pub(crate) const LEDGER_STORAGE_UNAVAILABLE: &str = "ledger_storage_unavailable";
pub(crate) const DEADLINE_PASSED: &str = "deadline";

const SILENT: &str = "silent";
const RECORDED: &str = "recorded";

const HOOK_KEY: &str = "hook";
const AGENT_ID_KEY: &str = "agent_id";
const MS_KEY: &str = "ms";

pub(crate) type Fields = Vec<(CodePointString, JsonValue)>;

// What one run reads from its process: the ledger directory, the log variable, the start-up
// directory and the clock.
pub(crate) struct Run<'a> {
    pub(crate) ledger: PathBuf,
    pub(crate) log: Option<&'a OsStr>,
    // None when the start-up directory cannot be read.
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) clock: &'a dyn Clock,
}

impl<'a> Run<'a> {
    pub(crate) fn of(env: &'a EnvConfig, clock: &'a dyn Clock) -> Self {
        Self {
            ledger: ledger::base(env),
            log: env.var(log::LOG_ENV),
            cwd: env.cwd().ok(),
            clock,
        }
    }

    // The payload's cwd, else the start-up directory.
    pub(crate) fn cwd(&self, fields: &Fields) -> Option<PathBuf> {
        let cwd = text_field(fields, "cwd").and_then(CodePointString::as_str);
        cwd.map(PathBuf::from)
            .or_else(|| self.cwd.map(Path::to_path_buf))
    }

    // The milliseconds since `started`, by the run's clock.
    pub(crate) fn ms_since(&self, started: Instant) -> u128 {
        self.clock
            .now()
            .saturating_duration_since(started)
            .as_millis()
    }
}

// What a run that decided did: a context for stdout, or a change to its state with no output.
pub(crate) enum Outcome {
    Context(String),
    Recorded(&'static str),
}

// What the log line records besides the decision.
#[derive(Default)]
pub(crate) struct Facts {
    pub(crate) session: Option<String>,
    pub(crate) agent: Option<String>,
    pub(crate) ms: Option<u128>,
}

// The context of a payload: the conversation and, in a subagent, the agent.
pub(crate) struct Context {
    pub(crate) session: String,
    pub(crate) agent: Option<String>,
}

// Fail open: a panic prints nothing, and every run logs one line.
pub(crate) fn answered(
    run: &Run<'_>,
    hook: &str,
    event_name: &str,
    stdout: &mut dyn Write,
    decide: impl FnOnce(&mut Facts) -> Result<Outcome, &'static str>,
) {
    let ts = log::now(run.clock);
    let mut facts = Facts::default();
    let decided = panic::catch_unwind(AssertUnwindSafe(|| decide(&mut facts)));
    let (event, reason) = match decided {
        Ok(Ok(Outcome::Context(context))) => match reply::written(stdout, event_name, &context) {
            EMITTED => (EMITTED, EMITTED),
            failed => (SILENT, failed),
        },
        Ok(Ok(Outcome::Recorded(reason))) => (RECORDED, reason),
        Ok(Err(reason)) => (SILENT, reason),
        Err(_) => (SILENT, INTERNAL),
    };
    // The log is the hook's only channel: a failed append has nowhere to be reported.
    drop(log::append(
        run.log,
        &entry(ts, hook, event, reason, &facts),
    ));
}

// The payload's object. An unreadable stdin and a document past a parse limit are malformed too.
pub(crate) fn fields(stdin: &mut dyn Read) -> Result<Fields, &'static str> {
    let Ok(Parsed::Value(mut document)) = parsed(stdin) else {
        return Err(MALFORMED_PAYLOAD);
    };
    match &mut document {
        JsonValue::Object(fields) => Ok(std::mem::take(fields)),
        _ => Err(MALFORMED_PAYLOAD),
    }
}

// A subagent's payload carries its parent's session id, so the agent id keeps the contexts apart.
// An agent id that is not a string could join two contexts, so it is malformed.
pub(crate) fn context(fields: &Fields, facts: &mut Facts) -> Result<Context, &'static str> {
    let session = text_field(fields, "session_id").and_then(CodePointString::as_str);
    let session = session.ok_or(NO_SESSION_ID)?.to_owned();
    facts.session = Some(session.clone());
    let agent = if get(fields, AGENT_ID_KEY).is_some() {
        let agent = text_field(fields, AGENT_ID_KEY).and_then(CodePointString::as_str);
        Some(agent.ok_or(MALFORMED_PAYLOAD)?.to_owned())
    } else {
        None
    };
    facts.agent.clone_from(&agent);
    Ok(Context { session, agent })
}

// The text of `key` in the payload's `tool_input`.
pub(crate) fn input_text<'a>(fields: &'a Fields, key: &str) -> Option<&'a str> {
    let input = object_field(fields, "tool_input")?;
    text_field(input, key).and_then(CodePointString::as_str)
}

fn entry(
    ts: Cow<'static, JsonValue>,
    hook: &str,
    event: &str,
    reason: &str,
    facts: &Facts,
) -> Entry<'static> {
    let text = |value: Option<&String>| {
        value.map_or(Cow::Owned(JsonValue::Null), |value| log::text(value))
    };
    let ms = facts
        .ms
        .map_or(JsonValue::Null, |ms| JsonValue::Int(ms.to_string()));
    vec![
        (key::TS, ts),
        (HOOK_KEY, log::text(hook)),
        (key::EVENT, log::text(event)),
        (key::REASON, log::text(reason)),
        (key::SESSION_ID, text(facts.session.as_ref())),
        (AGENT_ID_KEY, text(facts.agent.as_ref())),
        (MS_KEY, Cow::Owned(ms)),
    ]
}
