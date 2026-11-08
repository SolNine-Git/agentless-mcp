//! ledger-mark: after a trace call, a ledger entry in the call's context for the file of each
//! symbol card and file card that its targets resolve to, so a later read of the file gets no card.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use app::hooks::Hook;
use app::trace::{Card, CardKind, Outcome as TraceOutcome, Section, Trace, TraceRequest};
use app::{EnvConfig, Reading, Services, Session};
use json::{JsonValue, get};
use kernel::SystemClock;

use crate::ledger::{self, Claimed};
use crate::reply::INTERNAL;
use crate::repository;
use crate::run::{
    self, DEADLINE_PASSED, Facts, Fields, LEDGER_STORAGE_UNAVAILABLE, MALFORMED_PAYLOAD,
    NO_REPOSITORY, Outcome, Run,
};
use crate::search::object_field;
use crate::state;
use crate::worker::{self, Finished};

// The repository-relative files of the symbol and file cards that the targets resolve to.
pub(crate) type FilesFn = fn(&Services, &Path, &[String]) -> Result<Vec<String>, app::Error>;

const HOOK: &str = "ledger-mark";
const EVENT_NAME: &str = "PostToolUse";

const MARKED: &str = "marked";
const HELD: &str = "held";
const LEDGER_FULL: &str = "ledger_full";
const NO_CARD_FILE: &str = "no_card_file";
const RESOLVE_FAILED: &str = "resolve_failed";

/// Records the `PostToolUse` payload of a trace call on `stdin`: a ledger entry for the file of
/// each symbol and file card that its targets resolve to. It writes nothing and returns 0.
pub fn ledger_mark(env: &EnvConfig, services: &Services, stdin: &mut dyn Read) -> u8 {
    let clock = SystemClock;
    let hook = LedgerMark {
        run: Run::of(env, &clock),
        files: in_process,
        deadline: Hook::LedgerMark.deadline(),
    };
    answer(&hook, services, stdin, &mut std::io::sink());
    0
}

// One run: its process facts, the resolution and its deadline.
pub(crate) struct LedgerMark<'a> {
    pub(crate) run: Run<'a>,
    pub(crate) files: FilesFn,
    pub(crate) deadline: Duration,
}

pub(crate) fn answer(
    hook: &LedgerMark<'_>,
    services: &Services,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
) {
    run::answered(&hook.run, HOOK, EVENT_NAME, stdout, |facts| {
        decide(hook, services, stdin, facts)
    });
}

// The targets resolve as the server resolves them, with the one section per kind that costs
// least: a symbol's body and a file's declarations. A file target is traced a second time.
pub(crate) fn in_process(
    services: &Services,
    root: &Path,
    targets: &[String],
) -> Result<Vec<String>, app::Error> {
    let repo = app::Repo::load(services, root)?;
    let session = Session::open_facts(services, repo, false).session.value?;
    let reading = Reading::of(&session);
    let first = traced(services, &reading, targets, Section::Body)?;
    let mut files = card_files(&first);
    let file_targets: Vec<String> = (first.cards.iter())
        .filter(|traced| {
            matches!(
                traced.outcome,
                TraceOutcome::NoSection {
                    kind: CardKind::File,
                    ..
                }
            )
        })
        .map(|traced| traced.target.clone())
        .collect();
    if !file_targets.is_empty() {
        files.extend(card_files(&traced(
            services,
            &reading,
            &file_targets,
            Section::Declares,
        )?));
    }
    let mut seen = HashSet::new();
    files.retain(|file| seen.insert(file.clone()));
    Ok(files)
}

fn traced(
    services: &Services,
    reading: &Reading<'_>,
    targets: &[String],
    more: Section,
) -> Result<Trace, app::Error> {
    let request = TraceRequest {
        targets: targets.to_vec(),
        more: Some(more),
    };
    app::trace::trace(services, reading, &request).value
}

fn card_files(trace: &Trace) -> Vec<String> {
    let files = trace
        .cards
        .iter()
        .filter_map(|traced| match &traced.outcome {
            TraceOutcome::Card(Card::Symbol(card)) => Some(card.card.path.clone()),
            TraceOutcome::Card(Card::File(card)) => Some(card.path.clone()),
            _ => None,
        });
    files.collect()
}

// The tool_input, not the tool_response: the response's shape is not documented.
fn decide(
    hook: &LedgerMark<'_>,
    services: &Services,
    stdin: &mut dyn Read,
    facts: &mut Facts,
) -> Result<Outcome, &'static str> {
    let fields = run::fields(stdin)?;
    let context = run::context(&fields, facts)?;
    let targets = targets(&fields)?;
    let root = root(&hook.run, services, &fields).ok_or(NO_REPOSITORY)?;
    if !state::private_dir(&hook.run.ledger) {
        return Err(LEDGER_STORAGE_UNAVAILABLE);
    }
    let files = resolved(hook, services, root.clone(), targets, facts)?;
    let claims = files.iter().filter_map(|rel| {
        let canonical = io::resolve(&root.join(rel)).ok()?;
        let bytes = io::read_bounded(&canonical).ok()?;
        Some(ledger::claim(
            &hook.run.ledger,
            &context,
            &canonical,
            &bytes,
        ))
    });
    let claims: Vec<Claimed> = claims.collect();
    reason(&claims).map(Outcome::Recorded)
}

// A string or a list of strings, as the trace tool takes it.
fn targets(fields: &Fields) -> Result<Vec<String>, &'static str> {
    let input = object_field(fields, "tool_input").ok_or(MALFORMED_PAYLOAD)?;
    let text = |value: &JsonValue| match value {
        JsonValue::Str(text) => text.as_str().map(str::to_owned),
        _ => None,
    };
    let targets = match get(input, "target") {
        Some(JsonValue::Array(items)) => items.iter().map(text).collect(),
        Some(value) => text(value).map(|target| vec![target]),
        None => None,
    };
    targets
        .filter(|targets| !targets.is_empty())
        .ok_or(MALFORMED_PAYLOAD)
}

// The call's repo_root, else the repository of the call's directory.
fn root(run: &Run<'_>, services: &Services, fields: &Fields) -> Option<PathBuf> {
    if let Some(root) = run::input_text(fields, "repo_root") {
        return io::resolve(Path::new(root)).ok();
    }
    repository::root_of(services, &run.cwd(fields)?)
}

fn resolved(
    hook: &LedgerMark<'_>,
    services: &Services,
    root: PathBuf,
    targets: Vec<String>,
    facts: &mut Facts,
) -> Result<Vec<String>, &'static str> {
    let (services, files) = (services.clone(), hook.files);
    let started = hook.run.clock.now();
    let finished = worker::run(
        HOOK,
        move || files(&services, &root, &targets),
        hook.deadline,
    );
    facts.ms = Some(hook.run.ms_since(started));
    match finished {
        Ok(Finished::Done(files)) => Ok(files),
        Ok(Finished::TimedOut) => Err(DEADLINE_PASSED),
        Ok(Finished::Failed(_)) => Err(RESOLVE_FAILED),
        Ok(Finished::Panicked(_)) | Err(_) => Err(INTERNAL),
    }
}

// Marked when any claim is new; else the first reason that kept a claim out.
fn reason(claims: &[Claimed]) -> Result<&'static str, &'static str> {
    if claims.contains(&Claimed::New) {
        return Ok(MARKED);
    }
    let first = claims.iter().find(|claim| **claim != Claimed::Held);
    match first {
        None if claims.is_empty() => Err(NO_CARD_FILE),
        None => Err(HELD),
        Some(Claimed::Full) => Err(LEDGER_FULL),
        Some(_) => Err(LEDGER_STORAGE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests;
