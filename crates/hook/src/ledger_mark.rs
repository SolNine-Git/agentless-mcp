//! ledger-mark: after a trace call, a ledger entry in the call's context for the file of each
//! symbol card and file card that its targets resolve to, so a later read of the file gets no card.

use std::io::Read;
use std::path::{Path, PathBuf};

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

// The targets resolve as the server resolves them, and no card section is read.
pub(crate) fn in_process(
    services: &Services,
    root: &Path,
    targets: &[String],
) -> Result<Vec<String>, app::Error> {
    let repo = app::Repo::load(services, root)?;
    let session = Session::open_facts(services, repo, false).session.value?;
    app::trace::card_files(services, &Reading::of(&session), targets).value
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
    let root = repo_root(run, services, &fields, facts)?;
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
fn repo_root(
    run: &Run<'_>,
    services: &Services,
    fields: &Fields,
    facts: &mut Facts,
) -> Result<PathBuf, &'static str> {
    if let Some(root) = run::input_text(fields, "repo_root") {
        return io::resolve(Path::new(root)).map_err(|_| NO_REPOSITORY);
    }
    let cwd = run.cwd(fields).ok_or(NO_REPOSITORY)?;
    repository::root_of(services, &cwd)
        .map_err(|no_root| run::rootless(&no_root, NO_REPOSITORY, facts))
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
