//! The steps of one run: the parse, the refusals that come before any repository work, the
//! repository and its cache, the command, and the answer.

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use app::{EnvConfig, Services, Session};
use render::{Receipt, one_line};

use crate::Streams;
use crate::args::{self, Arg, Exit};
use crate::commands;
use crate::exit::{EXIT_OK, EXIT_USAGE, code_for, fail, report, write_answer};
use crate::invocation::{Command, Invocation, RepoFlags};
use crate::{lint, read};

const DEFAULT_COLUMNS: isize = 80;
// No line of these texts comes near this width, so a larger count changes nothing.
const MAX_COLUMNS: isize = 1 << 24;

pub(crate) fn run(
    args: &[OsString],
    env: &EnvConfig,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    let args: Vec<Arg> = args.iter().map(|arg| Arg::new(arg)).collect();
    let width = columns(env) - 2;
    match args::parse(&commands::top(), &args, width) {
        Ok(parsed) => execute(Invocation::new(parsed), services, streams),
        Err(Exit::Help(text)) => write_answer(streams, &text, "", EXIT_OK),
        Err(Exit::Usage(text)) => {
            report(streams.err, &text);
            EXIT_USAGE
        }
    }
}

fn execute(invocation: Invocation, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let counted = match with_counter(services, invocation.tiktoken) {
        Ok(counted) => counted,
        Err(error) => return fail(streams.err, &error.to_string(), code_for(&error)),
    };
    let services = counted.as_ref().unwrap_or(services);
    match invocation.command {
        Command::Unported(name) => {
            let message = format!(
                "this build does not run {name} yet. Run it with the Python agentless-mcp."
            );
            fail(streams.err, &message, EXIT_USAGE)
        }
        Command::Lint(request) => lint::run(&request, services, streams),
        Command::Capabilities(flags) => capabilities(&flags, services, streams),
        Command::Read(call) => read::run(&call, services, streams),
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

// Writes a rendered answer; a payload that does not serialize is a defect.
pub(crate) fn answer(
    streams: &mut Streams<'_>,
    rendered: Result<String, render::Error>,
    code: u8,
) -> u8 {
    match rendered {
        Ok(text) => write_answer(streams, &text, "", code),
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

// A decimal integer by the text crate's integer rule. A value past isize saturates, and
// MAX_COLUMNS caps it.
fn integer(text: &str) -> Option<isize> {
    let found = text::int_text(text)?;
    let value = found.digits().fold(0isize, |value, digit| {
        value.saturating_mul(10).saturating_add(isize::from(digit))
    });
    Some(if found.negative { -value } else { value })
}

#[cfg(test)]
mod tests;
