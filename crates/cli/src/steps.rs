//! The steps of one run: the parse, the refusals that come before any repository work, the
//! repository and its cache, the command, and the answer.

use std::ffi::OsString;
use std::path::Path;

use app::{EnvConfig, Services, Session};
use render::{Receipt, one_line};

use crate::Streams;
use crate::args::{self, Arg, Exit};
use crate::commands;
use crate::exit::{EXIT_OK, EXIT_USAGE, code_for, fail, report, write_answer};
use crate::invocation::{Command, Invocation, RepoFlags};
use crate::lint;

const TIKTOKEN_ABSENT: &str = "--token-counter tiktoken needs the 'tokens' extra, which is not \
    installed (No module named 'tiktoken'). Install it with: uv sync --extra tokens, or pip \
    install 'agentless-mcp[tokens]'.";
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
    if invocation.tiktoken {
        return fail(streams.err, TIKTOKEN_ABSENT, EXIT_USAGE);
    }
    match invocation.command {
        Command::Unported(name) => {
            let message = format!(
                "this build does not run {name} yet. Run it with the Python agentless-mcp."
            );
            fail(streams.err, &message, EXIT_USAGE)
        }
        Command::Lint(request) => lint::run(&request, services, streams),
        Command::Capabilities(flags) => capabilities(&flags, services, streams),
    }
}

fn capabilities(flags: &RepoFlags, services: &Services, streams: &mut Streams<'_>) -> u8 {
    let session = match open(flags, services, streams) {
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

// Resolves the repository, reports its note, and opens its cache, with each diagnostic on stderr.
pub(crate) fn open(
    flags: &RepoFlags,
    services: &Services,
    streams: &mut Streams<'_>,
) -> Result<Session, u8> {
    let repo = flags.repo.as_deref().map(Path::new);
    let repo = app::resolve(services, repo)
        .map_err(|error| fail(streams.err, &error.to_string(), code_for(&error)))?;
    let opening = Session::open(services, repo, flags.no_cache);
    let note = &opening.note;
    if !note.is_empty() {
        report(streams.err, &format!("agentless-mcp: {}\n", one_line(note)));
    }
    let opened = opening
        .session
        .map_err(|error| fail(streams.err, &error.to_string(), code_for(&error)))?;
    diagnostics(streams, &opened.diagnostics);
    Ok(opened.value)
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

// A decimal integer with an optional sign, surrounding whitespace, and single underscores
// between digits.
fn integer(text: &str) -> Option<isize> {
    let text = text::strip(text);
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let grouped = !digits.starts_with('_') && !digits.ends_with('_') && !digits.contains("__");
    if digits.is_empty() || !grouped {
        return None;
    }
    let mut value: isize = 0;
    for byte in digits.bytes().filter(|&b| b != b'_') {
        let digit = byte.is_ascii_digit().then(|| isize::from(byte - b'0'))?;
        value = value.saturating_mul(10).saturating_add(digit);
    }
    Some(if negative { -value } else { value })
}
