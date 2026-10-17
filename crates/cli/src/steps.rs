//! The steps of one run: the parse, the refusals that come before any repository work, the
//! repository and its cache, the command, and the answer.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use app::{EnvConfig, Noted, Repo, Services, Session};
use render::{Receipt, one_line};

use crate::Streams;
use crate::args::{self, Arg, Exit};
use crate::commands;
use crate::exit::{EXIT_OK, EXIT_USAGE, code_for, fail, report, write_answer};
use crate::invocation::{Command, Invocation, RepoFlags};
use crate::{lint, patches, read, validate, vote};

// The receipt's cache field for a command that opens no cache.
pub(crate) const NO_CACHE: &str = "none";

const DEFAULT_COLUMNS: isize = 80;
// No line of these texts comes near this width, so a larger count changes nothing.
const MAX_COLUMNS: isize = 1 << 24;

pub(crate) fn run(
    args: &[OsString],
    env: &EnvConfig,
    services: &Services,
    stdin: &mut dyn Read,
    streams: &mut Streams<'_>,
) -> u8 {
    let args: Vec<Arg> = args.iter().map(|arg| Arg::new(arg)).collect();
    let width = columns(env) - 2;
    match args::parse(&commands::top(), &args, width) {
        Ok(parsed) => {
            let mut run = Run { env, stdin };
            execute(Invocation::new(parsed), &mut run, services, streams)
        }
        Err(Exit::Help(text)) => write_answer(streams, &text, "", EXIT_OK),
        Err(Exit::Usage(text)) => {
            report(streams.err, &text);
            EXIT_USAGE
        }
    }
}

// The start-up environment of one run, and its stdin.
struct Run<'a> {
    env: &'a EnvConfig,
    stdin: &'a mut dyn Read,
}

fn execute(
    invocation: Invocation,
    run: &mut Run<'_>,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let counted = match with_counter(services, invocation.tiktoken) {
        Ok(counted) => counted,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let services = counted.as_ref().unwrap_or(services);
    match invocation.command {
        Command::Lint(request) => lint::run(&request, services, streams),
        Command::Capabilities(flags) => capabilities(&flags, services, streams),
        Command::Read(call) => read::run(&call, services, streams),
        Command::PatchParse(input) => patches::parse(&input, services, run.stdin, streams),
        Command::PatchEdit(call) => patches::edit(&call, services, run.stdin, streams),
        Command::Validate(call) => validate::run(&call, run.env, services, streams),
        Command::Vote(call) => vote::run(&call, services, streams),
    }
}

// The services of a run with `--token-counter tiktoken`: the cl100k_base counter in place of
// chars4. `None` keeps the start-up services.
fn with_counter(services: &Services, tiktoken: bool) -> Result<Option<Services>, app::Error> {
    if !tiktoken {
        return Ok(None);
    }
    let counter = Arc::new(app::Cl100k::new()?);
    Ok(Some(Services {
        counter,
        ..services.clone()
    }))
}

fn capabilities(flags: &RepoFlags, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let session = match open_with(flags, services, streams, Session::open_facts) {
        Ok(session) => session,
        Err(code) => return code,
    };
    let report = match app::capabilities(&session) {
        Ok(report) => report,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let receipt = Receipt::of(&session);
    let rendered = if flags.json {
        render::capabilities_json(&receipt, &report)
    } else {
        Ok(render::capabilities_text(&receipt, &report))
    };
    answer(streams, rendered, EXIT_OK)
}

// Resolves the repository, reports its note, and opens its tag cache, with each diagnostic on
// stderr.
pub(crate) fn open(
    flags: &RepoFlags,
    services: &Services,
    streams: &mut Streams<'_>,
) -> Result<Session, u8> {
    open_with(flags, services, streams, Session::open)
}

// As `open`, with the cache that `opener` opens: the tag cache or agentless.db.
pub(crate) fn open_with(
    flags: &RepoFlags,
    services: &Services,
    streams: &mut Streams<'_>,
    opener: fn(&Services, app::Repo, bool) -> app::Opening,
) -> Result<Session, u8> {
    let repo = flags.repo.as_deref().map(Path::new);
    let repo = app::resolve(services, repo)
        .map_err(|error| fail(streams.err, &error.to_string(), code_for(&error)))?;
    opened(streams, opener(services, repo, flags.no_cache))
}

// Reports the note and the warnings of a session's opening on stderr, then gives the session or
// the exit code of its failure.
pub(crate) fn opened(streams: &mut Streams<'_>, opening: app::Opening) -> Result<Session, u8> {
    let note = &opening.note;
    if !note.is_empty() {
        report(streams.err, &format!("agentless-mcp: {}\n", one_line(note)));
    }
    diagnostics(streams, &opening.session.diagnostics);
    opening
        .session
        .value
        .map_err(|error| fail(streams.err, &error.to_string(), code_for(&error)))
}

pub(crate) fn diagnostics(streams: &mut Streams<'_>, lines: &[String]) {
    for line in lines {
        report(streams.err, &format!("{line}\n"));
    }
}

// The diagnostics of a call that held SIGINT. When a SIGINT arrived while it was held, the call's
// error follows them, and then the process ends by SIGINT.
pub(crate) fn diagnostics_or_stop<T>(
    streams: &mut Streams<'_>,
    noted: &Noted<Result<T, app::Error>>,
) {
    diagnostics(streams, &noted.diagnostics);
    if io::interrupted() {
        if let Err(error) = &noted.value {
            fail(streams.err, &error.to_string(), code_for(error));
        }
        io::end_by_interrupt();
    }
}

// The repository of a command that opens no cache, and its git state.
pub(crate) struct Resolved {
    pub(crate) repo: Repo,
    pub(crate) snapshot: git::Snapshot,
}

impl Resolved {
    pub(crate) fn receipt(&self) -> Receipt<'_> {
        Receipt::new(&self.repo, &self.snapshot, NO_CACHE)
    }
}

// Resolves the repository with `resolver` and reads its git state, with the note on what is
// unknown on stderr.
pub(crate) fn resolved(
    repo: Option<&OsStr>,
    resolver: fn(&Services, Option<&Path>) -> Result<Repo, app::Error>,
    services: &Services,
    streams: &mut Streams<'_>,
) -> Result<Resolved, u8> {
    let repo = resolver(services, repo.map(Path::new))
        .map_err(|error| fail(streams.err, &error.to_string(), code_for(&error)))?;
    let enclosing = services.git.root(&repo.root);
    let snapshot = services.git.snapshot(&repo.root, enclosing.as_deref());
    if !snapshot.note.is_empty() {
        let note = format!("agentless-mcp: {}\n", one_line(&snapshot.note));
        report(streams.err, &note);
    }
    Ok(Resolved { repo, snapshot })
}

// Writes a rendered answer; a payload that does not serialize is a defect.
pub(crate) fn answer(
    streams: &mut Streams<'_>,
    rendered: Result<String, render::Error>,
    code: u8,
) -> u8 {
    answer_with(streams, rendered, "", code)
}

// Writes a rendered answer on stdout and `notes` on stderr.
pub(crate) fn answer_with(
    streams: &mut Streams<'_>,
    rendered: Result<String, render::Error>,
    notes: &str,
    code: u8,
) -> u8 {
    match rendered {
        Ok(text) => write_answer(streams, &text, notes, code),
        Err(error) => fail(streams.err, &error.to_string(), crate::EXIT_DOMAIN),
    }
}

// The terminal width that the help and usage texts wrap to: COLUMNS when it holds a positive
// integer, else 80.
fn columns(env: &EnvConfig) -> isize {
    let text = env.var("COLUMNS").and_then(|value| value.to_str());
    let count = text.and_then(integer).filter(|&count| count > 0);
    count.map_or(DEFAULT_COLUMNS, |count| count.min(MAX_COLUMNS))
}

// A decimal integer by the text crate's integer rule, of at most 4300 digits. A value past
// isize saturates, and MAX_COLUMNS caps it.
fn integer(text: &str) -> Option<isize> {
    let found = text::convertible_int(text)?;
    let value = found.digits().fold(0isize, |value, digit| {
        value.saturating_mul(10).saturating_add(isize::from(digit))
    });
    Some(if found.negative { -value } else { value })
}

#[cfg(test)]
mod tests;
