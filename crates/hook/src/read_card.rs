//! read-card: on the first read of a source file in a context, the file's card beside the read.
//! A read is a Read call, or a Bash command that is a read-before-edit view of one file.

use std::io::Read;
use std::path::{Path, PathBuf};

use app::search::is_source_path;
use app::trace::{Card, Outcome as TraceOutcome, TraceRequest, Traced};
use app::{Reading, Repo, Services, Session};
use io::Skipped;
use prompts::messages::READ_CARD_CONTEXT;
use render::TraceAnswer;
use text::CodePointString;

use crate::ledger::{self, Claimed};
use crate::repository;
use crate::run::{self, Facts, Fields, LEDGER_STORAGE_UNAVAILABLE, Outcome, Run};
use crate::search::{object_field, shell, text_field};

// The card of the file at a repository-relative path, or None when the trace gives no file card.
pub(crate) type CardFn = fn(&Services, &Path, &str) -> Result<Option<String>, app::Error>;

// The design's card cap, below the 10,000 characters of a documented context.
pub(crate) const CARD_CAP: usize = 1_000;

const READ: &str = "Read";
const BASH: &str = "Bash";

const NOT_A_READ: &str = "not_a_read";
const NO_FILE: &str = "no_file";
const NOT_SOURCE: &str = "not_source";
const OUTSIDE_GIT: &str = "outside_git";
const TOO_LARGE: &str = "too_large";
const HELD: &str = "held";
const LEDGER_FULL: &str = "ledger_full";
const RENDER_FAILED: &str = "render_failed";
const NO_FILE_CARD: &str = "no_file_card";

// The file card of the trace, as `trace` on the file's path gives it.
pub(crate) fn in_process(
    services: &Services,
    root: &Path,
    rel: &str,
) -> Result<Option<String>, app::Error> {
    let repo = Repo::load(services, root)?;
    let session = Session::open_facts(services, repo, false).session.value?;
    let request = TraceRequest {
        targets: vec![rel.to_owned()],
        more: None,
    };
    let trace = app::trace::trace(services, &Reading::of(&session), &request).value?;
    let is_file_card = matches!(
        trace.cards.as_slice(),
        [Traced {
            outcome: TraceOutcome::Card(Card::File(_)),
            ..
        }]
    );
    let answer = TraceAnswer {
        request: &request,
        trace: &trace,
    };
    Ok(is_file_card.then(|| render::trace_text(&answer)))
}

// The claim comes before the render, so a deadline or a crash uses the context's one try.
pub(crate) fn decide(
    run: &Run<'_>,
    card: CardFn,
    services: &Services,
    stdin: &mut dyn Read,
    facts: &mut Facts,
) -> Result<Outcome, &'static str> {
    let fields = run::fields(stdin)?;
    let context = run::context(&fields, facts)?;
    let read = read_file(run, &fields)?;
    let found = found(services, &read)?;
    match ledger::claim(&run.ledger, &context, &found.canonical, &found.content) {
        Claimed::New => {}
        Claimed::Held => return Err(HELD),
        Claimed::Full => return Err(LEDGER_FULL),
        Claimed::Unavailable => return Err(LEDGER_STORAGE_UNAVAILABLE),
    }
    let (services, root, rel) = (services.clone(), found.root, found.rel.clone());
    let finished = run.timed(move || card(&services, &root, &rel), facts);
    let card = run::finished(finished, RENDER_FAILED)?.ok_or(NO_FILE_CARD)?;
    let context = READ_CARD_CONTEXT.render((found.rel.as_str(), cut(&card).as_str()));
    Ok(Outcome::Context(context))
}

// The file that the call read, absolute: a relative path is below the call's directory.
fn read_file(run: &Run<'_>, fields: &Fields) -> Result<PathBuf, &'static str> {
    let tool = text_field(fields, "tool_name").and_then(CodePointString::as_str);
    let named = match tool {
        Some(READ) => run::input_text(fields, "file_path").map(PathBuf::from),
        Some(BASH) => bash_read(fields),
        _ => None,
    };
    let named = named.ok_or(NOT_A_READ)?;
    if named.is_absolute() {
        return Ok(named);
    }
    run.cwd(fields).map(|cwd| cwd.join(named)).ok_or(NOT_A_READ)
}

fn bash_read(fields: &Fields) -> Option<PathBuf> {
    let command = text_field(object_field(fields, "tool_input")?, "command")?;
    let file = shell::single_file_read(command)?;
    file.as_str().map(PathBuf::from)
}

// A source file that a repository holds, with its content as the ledger keys it.
struct Found {
    canonical: PathBuf,
    root: PathBuf,
    rel: String,
    content: Vec<u8>,
}

// The repository is the git top level of the file, not of the call's directory.
fn found(services: &Services, read: &Path) -> Result<Found, &'static str> {
    let canonical = io::resolve(read).map_err(|_| NO_FILE)?;
    if !canonical.is_file() {
        return Err(NO_FILE);
    }
    if !canonical.to_str().is_some_and(is_source_path) {
        return Err(NOT_SOURCE);
    }
    let dir = canonical.parent().ok_or(NO_FILE)?;
    let root = repository::root_of(services, dir).ok_or(OUTSIDE_GIT)?;
    let rel = canonical.strip_prefix(&root).ok().and_then(Path::to_str);
    let rel = rel.ok_or(OUTSIDE_GIT)?.to_owned();
    let content = io::read_bounded(&canonical).map_err(|skipped| match skipped {
        Skipped::TooLarge { .. } => TOO_LARGE,
        Skipped::Unreadable(_) => NO_FILE,
    })?;
    Ok(Found {
        canonical,
        root,
        rel,
        content,
    })
}

// Whole lines up to the cap. A first line longer than the cap is cut at the cap.
pub(crate) fn cut(card: &str) -> String {
    let card = card.strip_suffix('\n').unwrap_or(card);
    let mut kept = String::new();
    let mut count = 0;
    for line in card.split('\n') {
        let added = usize::from(count > 0) + line.chars().count();
        if count + added > CARD_CAP {
            break;
        }
        if count > 0 {
            kept.push('\n');
        }
        kept.push_str(line);
        count += added;
    }
    if kept.is_empty() {
        return card.chars().take(CARD_CAP).collect();
    }
    kept
}

#[cfg(test)]
mod tests;
