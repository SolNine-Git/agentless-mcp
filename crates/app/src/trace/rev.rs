//! A trace at a revision: the commit that `rev` names, the fast path that reads one blob per file
//! when each target is a symbol whose body or history the call asks for, and the walk and the
//! reader of the commit's tree that every other card reads.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use git::{Change, CommitId, Git, GitOutcome, RepoFile};
use graph::{Target, enclosing_symbol, joins_name_graph, parse_target};
use prompts::messages::{
    HISTORY_GIT_FAILED, TRACE_REV_NO_MATCH, TRACE_REV_NOT_A_NAME, TRACE_REV_SHALLOW,
    TRACE_REV_UNKNOWN,
};
use syntax::Language;

use super::symbol::refusal_text;
use super::{Card, Outcome, Revision, Section, SymbolTrace, TraceRequest, Traced};
use crate::expand::{EXPAND_BUDGET_TOKENS, body_card, fitted};
use crate::history::{committed_symbol, history_at};
use crate::ops::limits::{DEFAULT_HISTORY_BUDGET, DEFAULT_HISTORY_LIMIT};
use crate::scan::{AtCommit, Named, Tree, reads};
use crate::span::SymbolSpan;
use crate::{
    Error, HistoryRequest, HistoryResult, IntArg, Noted, Reading, Services, SymbolCard, symbol_card,
};

impl Named {
    pub(super) fn revision(&self) -> Revision {
        Revision {
            name: self.name.clone(),
            commit: self.commit.as_str().to_owned(),
        }
    }
}

// A name with ":" reads as a path and one with a leading "-" as an option, so git never sees
// either.
pub(super) fn named(services: &Services, root: &Path, name: &str) -> Result<Named, Error> {
    if name.contains(':') || name.starts_with('-') {
        return Err(refused(TRACE_REV_NOT_A_NAME.render(name)));
    }
    let note = match verified(services.git.verify(root, name)) {
        Ok(commit) => {
            let name = name.to_owned();
            return Ok(Named { name, commit });
        }
        Err(note) => note,
    };
    let mut message = TRACE_REV_UNKNOWN.render((name, &note));
    if services.git.is_shallow(root) == Ok(true) {
        message = format!("{message} {TRACE_REV_SHALLOW}");
    }
    Err(refused(message))
}

// The commit that `rev-parse --verify` printed, or git's note.
fn verified(outcome: GitOutcome) -> Result<CommitId, String> {
    let not_one = || git::Malformed::NotOneObjectName.to_string();
    let stdout = match outcome {
        GitOutcome::Printed(stdout) => stdout,
        GitOutcome::Capped(_) => return Err(not_one()),
        GitOutcome::Failed(failure) => return Err(failure.to_string()),
        GitOutcome::Unfinished(unfinished) => return Err(unfinished.to_string()),
    };
    let full = git::parse_commit(&stdout).map_err(|malformed| malformed.to_string())?;
    CommitId::parse(&full).ok_or_else(not_one)
}

fn refused(message: String) -> Error {
    kernel::Error::OperationFailed(message).into()
}

// The body or history cards of symbols that the commit holds; `None` sends the call to the scan
// of the commit, which also words each miss.
pub(super) fn fast_cards(
    services: &Services,
    reading: &Reading<'_>,
    request: &TraceRequest,
    named: &Named,
) -> Option<Noted<Result<Vec<Traced>, Error>>> {
    let more = request
        .more
        .filter(|more| matches!(more, Section::Body | Section::History))?;
    let mut blobs = HashMap::new();
    let mut found = Vec::new();
    for (at, target) in request.targets.iter().enumerate() {
        let span = match request.targets[..at].iter().position(|seen| seen == target) {
            Some(first) => Err(first),
            None => Ok(span_at(
                services,
                reading,
                &named.commit,
                target,
                &mut blobs,
            )?),
        };
        found.push((target, span));
    }
    let mut cards = Vec::new();
    for (target, span) in found {
        let outcome = match span {
            Err(first) => Outcome::Same(first),
            Ok(span) => match symbol_at(services, reading, &named.commit, &span, more) {
                Ok(card) => Outcome::Card(Card::Symbol(Box::new(card))),
                Err(error) => return Some(Noted::new(Err(error))),
            },
        };
        let target = target.clone();
        cards.push(Traced { target, outcome });
    }
    Some(Noted::new(Ok(cards)))
}

fn symbol_at(
    services: &Services,
    reading: &Reading<'_>,
    commit: &CommitId,
    span: &SymbolSpan,
    more: Section,
) -> Result<SymbolTrace, Error> {
    let mut traced = SymbolTrace {
        card: symbol_card(span.language, &span.path, &span.symbol, String::new()),
        callers: None,
        callees: None,
        tests: None,
        similar: None,
        change: None,
        history: None,
        body: None,
    };
    if more == Section::Body {
        traced.body = Some(Ok(fitted_body(services, span)));
    } else {
        let request = HistoryRequest {
            target: traced.card.stable_id.clone(),
            limit: IntArg::from_usize(DEFAULT_HISTORY_LIMIT),
            budget: IntArg::from_usize(DEFAULT_HISTORY_BUDGET),
        };
        let history = history_at(services, reading, &request, commit, span);
        traced.history = Some(refusal_text(history)?);
    }
    Ok(traced)
}

// The card with its numbered body, cut to the budget of one expansion.
fn fitted_body(services: &Services, span: &SymbolSpan) -> SymbolCard {
    let card = body_card(span);
    let mut fitted = fitted(
        &*services.counter,
        vec![card],
        Vec::new(),
        EXPAND_BUDGET_TOKENS,
    );
    fitted.cards.pop().expect("one card fits any budget")
}

// The symbol that a stable id or a `path:line` names, as `commit` has it; `None` for any other
// target, a file whose blob git does not print, or a symbol that the file does not hold.
fn span_at(
    services: &Services,
    reading: &Reading<'_>,
    commit: &CommitId,
    target: &str,
    blobs: &mut HashMap<String, Option<Vec<u8>>>,
) -> Option<SymbolSpan> {
    let (path, wanted) = match parse_target(target) {
        Target::Id(id, _) => (id.path().to_owned(), Wanted::Id(id)),
        Target::Line(path, line) if !path.starts_with(['/', '.']) => {
            (path.to_owned(), Wanted::Line(line))
        }
        Target::Line(..) | Target::Named(_) => return None,
    };
    let language = Language::from_path(&path).filter(|language| joins_name_graph(*language))?;
    if let Wanted::Id(id) = &wanted
        && id.prefix() != language.extraction().prefix
    {
        return None;
    }
    let bytes = (blobs.entry(path.clone()))
        .or_insert_with(|| blob(&services.git, &reading.repo.root, commit, &path))
        .as_deref()?;
    let at = (reading.facts, reading.deadline);
    let found = committed_symbol((&path, language), bytes, at, |symbols| match &wanted {
        Wanted::Id(id) => (symbols.iter()).find(|symbol| symbol.id_qualname() == id.qualname()),
        Wanted::Line(line) => enclosing_symbol(symbols, *line),
    });
    found.ok().flatten()
}

// What a fast-path target names in its file.
enum Wanted {
    Id(facts::StableId),
    Line(u32),
}

fn blob(git: &Git, root: &Path, commit: &CommitId, path: &str) -> Option<Vec<u8>> {
    match git.blob(root, commit.as_str(), path) {
        GitOutcome::Printed(bytes) => Some(bytes),
        GitOutcome::Capped(_) | GitOutcome::Failed(_) | GitOutcome::Unfinished(_) => None,
    }
}

// The commits on the lines of `request`'s target as the commit has them.
pub(super) fn history_of(
    services: &Services,
    reading: &Reading<'_>,
    request: &HistoryRequest,
    named: &Named,
) -> Result<HistoryResult, Error> {
    let span = span_at(
        services,
        reading,
        &named.commit,
        &request.target,
        &mut HashMap::new(),
    );
    let Some(span) = span else {
        return Err(refused(no_match(&request.target, named)));
    };
    history_at(services, reading, request, &named.commit, &span)
}

// The body of `target` as the commit has it, or why it was not read.
pub(super) fn body_of(
    services: &Services,
    reading: &Reading<'_>,
    named: &Named,
    target: &str,
) -> Result<SymbolCard, String> {
    let span = span_at(
        services,
        reading,
        &named.commit,
        target,
        &mut HashMap::new(),
    );
    let span = span.ok_or_else(|| no_match(target, named))?;
    Ok(fitted_body(services, &span))
}

fn no_match(target: &str, named: &Named) -> String {
    TRACE_REV_NO_MATCH.render((&named.name, named.short(), target))
}

// The files of the commit's tree, and the reader that takes each file's bytes from disk unless
// they differ from the commit's. The walk's warnings come with either outcome.
pub(super) fn walked_at(
    services: &Services,
    root: &Path,
    named: &Named,
) -> Noted<Result<(Vec<RepoFile>, Tree), Error>> {
    let git = &services.git;
    let walk = git.tree_walk(root, named.commit.as_str(), reads);
    let diagnostics = walk.warnings.iter().map(ToString::to_string).collect();
    let value = walk
        .files
        .map_err(|refusal| kernel::Error::from(refusal).into());
    let value = value.and_then(|files| {
        let changes = changed(git.changes(root, named.commit.as_str()))?;
        let at = AtCommit::new(git.clone(), named.clone(), changes.as_deref(), &files);
        Ok((files, Tree::At(Arc::new(at))))
    });
    Noted { value, diagnostics }
}

// The paths that differ from the commit, or `None` for a listing past its cap: it names many
// changes, and every file then reads as a blob.
fn changed(outcome: GitOutcome) -> Result<Option<Vec<Change>>, Error> {
    let note = match outcome {
        GitOutcome::Printed(raw) => match git::parse_changes(&raw) {
            Ok(changes) => return Ok(Some(changes)),
            Err(malformed) => malformed.to_string(),
        },
        GitOutcome::Capped(_) => return Ok(None),
        GitOutcome::Failed(failure) => failure.to_string(),
        GitOutcome::Unfinished(unfinished) => unfinished.to_string(),
    };
    Err(refused(HISTORY_GIT_FAILED.render(&note)))
}

#[cfg(test)]
mod tests;
