//! ledger-mark: after a trace call, a ledger entry in the call's context for the file of each
//! symbol card and file card that its targets resolve to, so a later read of the file gets no card.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use app::trace::{Card, CardKind, Outcome as TraceOutcome, Section, Trace, TraceRequest};
use app::{Reading, Services, Session};
use json::{JsonValue, get};

use crate::ledger::{self, Claimed};
use crate::repository;
use crate::run::{
    self, Facts, Fields, LEDGER_STORAGE_UNAVAILABLE, MALFORMED_PAYLOAD, NO_REPOSITORY, Outcome, Run,
};
use crate::search::object_field;
use crate::state;

// The repository-relative files of the symbol and file cards that the targets resolve to.
pub(crate) type FilesFn = fn(&Services, &Path, &[String]) -> Result<Vec<String>, app::Error>;

const MARKED: &str = "marked";
const HELD: &str = "held";
const LEDGER_FULL: &str = "ledger_full";
const NO_CARD_FILE: &str = "no_card_file";
const RESOLVE_FAILED: &str = "resolve_failed";

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
pub(crate) fn decide(
    run: &Run<'_>,
    files: FilesFn,
    services: &Services,
    stdin: &mut dyn Read,
    facts: &mut Facts,
) -> Result<Outcome, &'static str> {
    let fields = run::fields(stdin)?;
    let context = run::context(&fields, facts)?;
    let targets = targets(&fields)?;
    let root = repo_root(run, services, &fields).ok_or(NO_REPOSITORY)?;
    if !state::private_dir(&run.ledger) {
        return Err(LEDGER_STORAGE_UNAVAILABLE);
    }
    let (services, below) = (services.clone(), root.clone());
    let finished = run.timed(move || files(&services, &below, &targets), facts);
    let claims = run::finished(finished, RESOLVE_FAILED)?
        .into_iter()
        .filter_map(|rel| {
            let canonical = io::resolve(&root.join(rel)).ok()?;
            let bytes = io::read_bounded(&canonical).ok()?;
            Some(ledger::claim(&run.ledger, &context, &canonical, &bytes))
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
fn repo_root(run: &Run<'_>, services: &Services, fields: &Fields) -> Option<PathBuf> {
    if let Some(root) = run::input_text(fields, "repo_root") {
        return io::resolve(Path::new(root)).ok();
    }
    repository::root_of(services, &run.cwd(fields)?)
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
