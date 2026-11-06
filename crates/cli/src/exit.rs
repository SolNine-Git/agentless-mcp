//! Exit codes, the one-line failure report, and the writes of an answer to the two streams.

use std::io::{self, Write};
use std::panic::PanicHookInfo;

use kernel::Error as Kind;
use prompts::messages::{HOOK_USAGE, WALK_REFUSED_CLI};
use render::one_line;

use crate::Streams;

pub(crate) const EXIT_OK: u8 = 0;
/// The exit code of a run that the repository or a defect stopped.
pub const EXIT_DOMAIN: u8 = 1;
pub(crate) const EXIT_USAGE: u8 = 2;
pub(crate) const EXIT_GATE: u8 = 3;
/// The exit code of a run whose output its reader did not take.
pub const EXIT_CLOSED_STDOUT: u8 = 120;

const PREFIX: &str = "agentless-mcp: ";

// An answer that holds what the caller asked for exits 0, and one that does not exits 1.
pub(crate) const fn domain_code(ok: bool) -> u8 {
    if ok { EXIT_OK } else { EXIT_DOMAIN }
}

// The one-line report of an error that stopped a run, and its exit code. A walk bound refusal
// names the controls that the command line has.
pub(crate) fn failed(err: &mut dyn Write, error: &app::Error) -> u8 {
    let message = match error {
        app::Error::Kind(Kind::WalkBoundExceeded(reason)) => WALK_REFUSED_CLI.render(reason),
        other => other.to_string(),
    };
    fail(err, &message, code_for(error))
}

// A refusal of the request itself exits 2; every other error that stops a run exits 1.
fn code_for(error: &app::Error) -> u8 {
    let refused = matches!(
        error,
        app::Error::Kind(
            Kind::SecurityRefusal(_)
                | Kind::RepoResolution(_)
                | Kind::InputUnreadable(_)
                | Kind::RevisionUnresolved(_)
        )
    );
    if refused { EXIT_USAGE } else { EXIT_DOMAIN }
}

pub(crate) fn fail(err: &mut dyn Write, message: &str, code: u8) -> u8 {
    report(err, &format!("{PREFIX}{}\n", one_line(message)));
    code
}

// Stderr is the last channel: a failed write there has nowhere to go, and the exit code still
// carries the outcome.
pub(crate) fn report(err: &mut dyn Write, text: &str) {
    if !text.is_empty() {
        let written = err.write_all(text.as_bytes()).and_then(|()| err.flush());
        drop(written);
    }
}

/// Writes `out` to stdout and `err` to stderr, and returns `code`, or 120 when stdout is closed.
pub fn write_answer(streams: &mut Streams<'_>, out: &str, err: &str, code: u8) -> u8 {
    let written = streams.out.write_all(out.as_bytes());
    let written = written.and_then(|()| streams.out.flush());
    report(streams.err, err);
    match written {
        Ok(()) => code,
        Err(error) => {
            report(streams.err, &flush_failure(&error));
            EXIT_CLOSED_STDOUT
        }
    }
}

// The report of an answer that stdout did not take: the error class for the errno, then the
// errno and its message.
fn flush_failure(error: &io::Error) -> String {
    format!(
        "Exception ignored on flushing sys.stdout:\n{}\n",
        ::io::os_error_line(error)
    )
}

/// Writes the refusal of a `hook` command line that names none of the four hooks, and returns 2.
pub fn refuse_hook(streams: &mut Streams<'_>) -> u8 {
    fail(streams.err, HOOK_USAGE, EXIT_USAGE)
}

/// Returns the one stderr line that reports a panic.
#[must_use]
pub fn internal_error_line(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("a panic without a message");
    let place = info.location().map_or(String::new(), |at| {
        format!(" ({}:{})", at.file(), at.line())
    });
    format!(
        "{PREFIX}{}\n",
        one_line(&format!("internal error: {message}{place}"))
    )
}

/// Returns the one stderr line that reports an unreadable start-up directory.
#[must_use]
pub fn cwd_failure_line(error: &io::Error) -> String {
    let shown = ::io::os_error_class_text(error);
    format!(
        "{PREFIX}{}\n",
        one_line(&format!("cannot read the current directory: {shown}"))
    )
}

#[cfg(test)]
mod tests {
    use super::{EXIT_DOMAIN, EXIT_USAGE, failed};

    fn reported(error: kernel::Error) -> (u8, String) {
        let mut err = Vec::new();
        let code = failed(&mut err, &app::Error::Kind(error));
        (code, String::from_utf8(err).expect("a UTF-8 report"))
    }

    #[test]
    fn a_walk_bound_refusal_names_what_the_command_line_can_do() {
        let reason = "walk refused: more than 2 files under /r".to_owned();
        let expected = "agentless-mcp: walk refused: more than 2 files under /r. No flag changes a \
                        walk bound. Set --repo to a subdirectory, and the walk covers only that \
                        subtree.\n";
        let refused = reported(kernel::Error::WalkBoundExceeded(reason));
        assert_eq!(refused, (EXIT_DOMAIN, expected.to_owned()));
        let other = reported(kernel::Error::RepoResolution(
            "not a directory: /r".to_owned(),
        ));
        assert_eq!(
            other,
            (
                EXIT_USAGE,
                "agentless-mcp: not a directory: /r\n".to_owned()
            )
        );
    }
}
