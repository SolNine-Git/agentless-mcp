//! The prompt map: on the first prompt of a session that names files or symbols of the
//! repository, one sentence of context that names the files the map ranks first for them.

mod seeds;

use std::borrow::Cow;
use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Duration;

use app::{EnvConfig, IntArg, MapRequest, MapResult, Reading, Repo, Services, Session};
use git::Toplevel;
use io::Granularity;
use json::JsonValue;
use kernel::{Clock, SystemClock};

use crate::search::log::{self, Entry, key};
use crate::search::{Parsed, parsed, text_field};
use crate::text::sha256_hex;
use crate::worker::{self, Finished};

pub(crate) type MapFn = fn(&Services, &Path, &MapRequest) -> Result<MapResult, app::Error>;

const MARKER_DIR: &str = "agentless_prompt_map";
const TEMPORARY_DIR: &str = "/tmp";
const DEADLINE: Duration = Duration::from_secs(5);
// The erosion gate's git timeout.
const GIT_TIMEOUT: Duration = Duration::from_secs(2);
const THREAD: &str = "prompt-map";
const HOOK: &str = "prompt-map";
const EVENT_NAME: &str = "UserPromptSubmit";

const EMITTED: &str = "emitted";
const SILENT: &str = "silent";
const ALREADY_ENRICHED: &str = "already_enriched";
const MALFORMED_PAYLOAD: &str = "malformed_payload";
const NO_SESSION_ID: &str = "no_session_id";
const NO_PROMPT: &str = "no_prompt";
const NO_REPOSITORY: &str = "no_repository";
const NO_SEED: &str = "no_seed";
const NO_SEED_RESOLVED: &str = "no_seed_resolved";
const DEADLINE_PASSED: &str = "deadline";
const MAP_FAILED: &str = "map_failed";
const TOO_LONG: &str = "too_long";
const MARKER_STORAGE_UNAVAILABLE: &str = "marker_storage_unavailable";
const INTERNAL: &str = "internal";

const HOOK_KEY: &str = "hook";
const SEEDS_KEY: &str = "seeds";
const FILES_KEY: &str = "files";
const MS_KEY: &str = "ms";

/// Answers the `UserPromptSubmit` payload on `stdin` with one JSON object on `stdout` that names
/// the map's first files for the prompt's names, or with nothing, and returns 0.
pub fn prompt_map(
    env: &EnvConfig,
    services: &Services,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) -> u8 {
    let clock = SystemClock;
    let hook = Hook {
        markers: marker_dir(env),
        log: env.var(log::LOG_ENV),
        cwd: env.cwd().ok(),
        clock: &clock,
        map: in_process,
        deadline: DEADLINE,
    };
    answer(&hook, services, stdin, stdout);
    0
}

// What one run reads from its process: the marker directory, the log variable, the start-up
// directory, the clock, the map and its deadline.
pub(crate) struct Hook<'a> {
    pub(crate) markers: PathBuf,
    pub(crate) log: Option<&'a OsStr>,
    // None when the start-up directory cannot be read.
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) clock: &'a dyn Clock,
    pub(crate) map: MapFn,
    pub(crate) deadline: Duration,
}

// TMPDIR when it is set and not empty, else /tmp; never the search gate's directory.
fn marker_dir(env: &EnvConfig) -> PathBuf {
    let base = env.var("TMPDIR").filter(|value| !value.is_empty());
    base.map_or_else(|| PathBuf::from(TEMPORARY_DIR), PathBuf::from)
        .join(MARKER_DIR)
}

// The map of one prompt, as the command line's `map` makes it.
fn in_process(
    services: &Services,
    root: &Path,
    request: &MapRequest,
) -> Result<MapResult, app::Error> {
    let repo = Repo::load(services, root)?;
    let session = Session::open_facts(services, repo, false).session.value?;
    app::map(services, &Reading::of(&session), request).value
}

// What the log line records besides the decision.
#[derive(Default)]
struct Facts {
    session: Option<String>,
    seeds: Option<Vec<String>>,
    files: Option<Vec<String>>,
    ms: Option<u128>,
}

// Fail open: a panic prints nothing, and every run logs one line.
pub(crate) fn answer(
    hook: &Hook<'_>,
    services: &Services,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) {
    let ts = log::now(hook.clock);
    let mut facts = Facts::default();
    let decided = panic::catch_unwind(AssertUnwindSafe(|| {
        decide(hook, services, stdin, &mut facts)
    }));
    let reason = match decided {
        Ok(Ok(context)) => written(stdout, &context),
        Ok(Err(reason)) => reason,
        Err(_) => INTERNAL,
    };
    // The log is the hook's only channel: a failed append has nowhere to be reported.
    drop(log::append(hook.log, &entry(ts, &facts, reason)));
}

fn written(stdout: &mut dyn Write, context: &str) -> &'static str {
    let reply = JsonValue::Object(vec![(
        "hookSpecificOutput".into(),
        JsonValue::Object(vec![
            ("hookEventName".into(), JsonValue::Str(EVENT_NAME.into())),
            ("additionalContext".into(), JsonValue::Str(context.into())),
        ]),
    )]);
    let text = json::value_text(&reply);
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => EMITTED,
        Err(_) => INTERNAL,
    }
}

// The steps in order. A prompt with no seed writes no marker, so a later prompt can still emit.
fn decide(
    hook: &Hook<'_>,
    services: &Services,
    stdin: &mut dyn Read,
    facts: &mut Facts,
) -> Result<String, &'static str> {
    let document = parsed(stdin);
    let Ok(Parsed::Value(JsonValue::Object(fields))) = &document else {
        return Err(MALFORMED_PAYLOAD);
    };
    let session = text_field(fields, "session_id").and_then(|session| session.as_str());
    let session = session.ok_or(NO_SESSION_ID)?;
    facts.session = Some(session.to_owned());
    let prompt = text_field(fields, "prompt").and_then(|prompt| prompt.as_str());
    let prompt = prompt.ok_or(NO_PROMPT)?;
    let cwd = text_field(fields, "cwd").and_then(|cwd| cwd.as_str());
    let cwd = cwd.map(Path::new).or(hook.cwd).ok_or(NO_REPOSITORY)?;
    let root = repository_root(services, cwd).ok_or(NO_REPOSITORY)?;
    let found = seeds::seeds(prompt, &|rel| root.join(rel).is_file());
    facts.seeds = Some(found.clone());
    if found.is_empty() {
        return Err(NO_SEED);
    }
    claim(&hook.markers, session)?;
    let result = mapped(hook, services, root, found.clone(), facts)?;
    context(&result, &found, facts)
}

fn repository_root(services: &Services, cwd: &Path) -> Option<PathBuf> {
    let Toplevel::Found(stdout) = services.git.toplevel(cwd, GIT_TIMEOUT) else {
        return None;
    };
    let shown = String::from_utf8(stdout).ok()?;
    io::resolve(Path::new(text::strip(&shown))).ok()
}

// The session's one try: the marker is written before the map runs, so a deadline or a crash
// uses it too. Another user can plant entries in a shared directory, so only a private one counts.
fn claim(markers: &Path, session: &str) -> Result<(), &'static str> {
    io::create_missing_owner_dir(markers).map_err(|_| MARKER_STORAGE_UNAVAILABLE)?;
    if !matches!(io::is_private_directory(markers), Ok(true)) {
        return Err(MARKER_STORAGE_UNAVAILABLE);
    }
    let marker = markers.join(sha256_hex(session.as_bytes()));
    match io::write_new_file(&marker, b"") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists && is_plain_file(&marker) => {
            Err(ALREADY_ENRICHED)
        }
        Err(_) => Err(MARKER_STORAGE_UNAVAILABLE),
    }
}

fn is_plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn mapped(
    hook: &Hook<'_>,
    services: &Services,
    root: PathBuf,
    focus: Vec<String>,
    facts: &mut Facts,
) -> Result<MapResult, &'static str> {
    let request = MapRequest {
        focus,
        budget: None,
        max_files: Some(IntArg::from_usize(seeds::MAX_FILES)),
        granularity: Some(Granularity::File),
    };
    let (services, map) = (services.clone(), hook.map);
    let started = hook.clock.now();
    let finished = worker::run(
        THREAD,
        move || map(&services, &root, &request),
        hook.deadline,
    );
    facts.ms = Some(
        hook.clock
            .now()
            .saturating_duration_since(started)
            .as_millis(),
    );
    match finished {
        Ok(Finished::Done(result)) => Ok(result),
        Ok(Finished::TimedOut) => Err(DEADLINE_PASSED),
        Ok(Finished::Failed(_)) => Err(MAP_FAILED),
        Ok(Finished::Panicked(_)) | Err(_) => Err(INTERNAL),
    }
}

// The names are the seeds that the map resolved. A map that resolved none gives no sentence.
fn context(
    result: &MapResult,
    found: &[String],
    facts: &mut Facts,
) -> Result<String, &'static str> {
    let files: Vec<String> = (result.files.iter())
        .take(seeds::MAX_FILES)
        .map(|file| file.path.clone())
        .collect();
    facts.files = Some(files.clone());
    let names: Vec<String> = (found.iter())
        .filter(|seed| !result.unresolved_seeds.contains(seed))
        .cloned()
        .collect();
    if result.seeds.is_empty() || names.is_empty() || files.is_empty() {
        return Err(NO_SEED_RESOLVED);
    }
    seeds::context_text(&names, &files).ok_or(TOO_LONG)
}

fn entry(ts: Cow<'static, JsonValue>, facts: &Facts, reason: &'static str) -> Entry<'static> {
    let event = if reason == EMITTED { EMITTED } else { SILENT };
    vec![
        (key::TS, ts),
        (
            key::SESSION_ID,
            optional(facts.session.as_deref().map(log::text)),
        ),
        (HOOK_KEY, log::text(HOOK)),
        (key::EVENT, log::text(event)),
        (key::REASON, log::text(reason)),
        (SEEDS_KEY, optional(facts.seeds.as_deref().map(texts))),
        (FILES_KEY, optional(facts.files.as_deref().map(texts))),
        (
            MS_KEY,
            optional(
                facts
                    .ms
                    .map(|ms| Cow::Owned(JsonValue::Int(ms.to_string()))),
            ),
        ),
    ]
}

fn optional(value: Option<Cow<'static, JsonValue>>) -> Cow<'static, JsonValue> {
    value.unwrap_or(Cow::Owned(JsonValue::Null))
}

fn texts(items: &[String]) -> Cow<'static, JsonValue> {
    let items = items
        .iter()
        .map(|item| JsonValue::Str(item.as_str().into()));
    Cow::Owned(JsonValue::Array(items.collect()))
}

#[cfg(test)]
mod tests;
