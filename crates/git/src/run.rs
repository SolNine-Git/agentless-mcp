//! The one way this crate starts git: the hardening argv, the scrubbed environment, and the outcome
//! of one bounded call.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::Duration;

// Each entry pins one setting that a repository's own config could otherwise use to run a program
// or to change what git prints.
const HARDENING_PREFIX: [&str; 11] = [
    "--no-optional-locks",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "core.pager=cat",
    "-c",
    "diff.external=",
    "-c",
    "core.quotePath=false",
    "-c",
    "log.showSignature=false",
];

// These two move where git reads config from, never which repository it reads.
const CONFIG_KEPT: [&str; 2] = ["GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM"];

// A note quotes only the first stderr line, so the rest is read past.
const MAX_STDERR_BYTES: usize = 65_536;

/// Runs git with the hardening argv prefix, a scrubbed environment, and a bound on every call.
#[derive(Clone)]
pub struct Git {
    env: Vec<(OsString, OsString)>,
}

/// How one bounded git call ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitOutcome {
    /// Git exited 0, and this is its whole stdout.
    Printed(Vec<u8>),
    /// Git printed more than the call's cap and was killed. This is the first cap bytes.
    Capped(Vec<u8>),
    /// Git exited with a status other than 0.
    Failed(Failure),
    /// Git gave no exit status: it timed out, did not start, or was lost.
    Unfinished(Unfinished),
}

/// A git call that exited with a status other than 0. It displays as the note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// The git subcommand.
    pub subcommand: &'static str,
    /// The exit status, or the negative signal number that killed git.
    pub code: i32,
    /// The first line of stderr, or "no detail".
    pub detail: String,
}

/// A git call with no exit status. It displays as the note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unfinished {
    /// The deadline passed and git was killed.
    TimedOut {
        /// The git subcommand.
        subcommand: &'static str,
        /// The deadline in seconds, as the note shows it.
        after: &'static str,
    },
    /// No git program was found.
    NotInstalled,
    /// Git could not be started, or a pipe read or the wait failed after the start.
    NotRun {
        /// The git subcommand.
        subcommand: &'static str,
        /// The operating system's message.
        reason: String,
    },
}

// One git command line after the hardening prefix: `-c key=value` arguments, which go before
// `-C`, then the subcommand and its arguments.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Call<'a> {
    pub(crate) config: &'a [&'a str],
    pub(crate) subcommand: &'static str,
    pub(crate) rest: &'a [&'a OsStr],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) seconds: u64,
    // The deadline as the timeout note shows it.
    pub(crate) shown: &'static str,
    pub(crate) cap: usize,
}

impl Git {
    /// Keeps `base` without its `GIT_` names, apart from the two config names, and sets `LC_ALL=C`.
    pub fn new(base: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        let mut env: Vec<_> = base
            .into_iter()
            .filter(|(name, _)| inherited(name))
            .collect();
        // The C locale keeps git's failure text in the spelling the callers classify.
        env.push((OsString::from("LC_ALL"), OsString::from("C")));
        Self { env }
    }

    pub(crate) fn run(
        &self,
        dir: &Path,
        subcommand: &'static str,
        rest: &[&OsStr],
        limits: Limits,
    ) -> GitOutcome {
        let call = Call {
            config: &[],
            subcommand,
            rest,
        };
        self.run_call(dir, &call, limits)
    }

    pub(crate) fn run_call(&self, dir: &Path, call: &Call<'_>, limits: Limits) -> GitOutcome {
        classify(
            call.subcommand,
            limits,
            io::run(&self.call_spec(dir, call, limits)),
        )
    }

    pub(crate) fn spec(
        &self,
        dir: &Path,
        subcommand: &'static str,
        rest: &[&OsStr],
        limits: Limits,
    ) -> io::Spec {
        let call = Call {
            config: &[],
            subcommand,
            rest,
        };
        self.call_spec(dir, &call, limits)
    }

    pub(crate) fn call_spec(&self, dir: &Path, call: &Call<'_>, limits: Limits) -> io::Spec {
        let fixed = HARDENING_PREFIX
            .iter()
            .chain(call.config)
            .map(OsString::from);
        let target = [OsString::from("-C"), dir.as_os_str().to_owned()];
        let call = std::iter::once(OsString::from(call.subcommand))
            .chain(call.rest.iter().map(|argument| (*argument).to_owned()));
        io::Spec {
            program: OsString::from("git"),
            args: fixed.chain(target).chain(call).collect(),
            // `-C` names the repository, so a missing directory reaches git and gets its exit 128.
            cwd: PathBuf::from("."),
            env: self.env.clone(),
            timeout: Duration::from_secs(limits.seconds),
            stdout_cap: limits.cap,
            stderr_cap: MAX_STDERR_BYTES,
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            subcommand,
            code,
            detail,
        } = self;
        write!(f, "git {subcommand} exited {code}: {detail}")
    }
}

impl fmt::Display for Unfinished {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut { subcommand, after } => {
                write!(f, "git {subcommand} timed out after {after}s")
            }
            Self::NotInstalled => {
                f.write_str("git is not installed, so repository state is unknown")
            }
            Self::NotRun { subcommand, reason } => {
                write!(f, "git {subcommand} could not be run: {reason}")
            }
        }
    }
}

fn inherited(name: &OsStr) -> bool {
    if name == "LC_ALL" {
        return false;
    }
    !name.as_encoded_bytes().starts_with(b"GIT_") || CONFIG_KEPT.iter().any(|kept| name == *kept)
}

pub(crate) fn classify(
    subcommand: &'static str,
    limits: Limits,
    outcome: io::Outcome,
) -> GitOutcome {
    let unfinished =
        |reason: String| GitOutcome::Unfinished(Unfinished::NotRun { subcommand, reason });
    match outcome {
        io::Outcome::Exited {
            status,
            stdout,
            stderr,
        } => exited(subcommand, status, stdout, &stderr),
        io::Outcome::Truncated { stdout } => GitOutcome::Capped(stdout),
        io::Outcome::TimedOut => GitOutcome::Unfinished(Unfinished::TimedOut {
            subcommand,
            after: limits.shown,
        }),
        io::Outcome::NotRun(io::NotRun::ProgramNotFound(_)) => {
            GitOutcome::Unfinished(Unfinished::NotInstalled)
        }
        io::Outcome::NotRun(not_run) => unfinished(not_run.to_string()),
        // A lost child has no exit status, so it reads as one that never ran. NotRun renders the
        // operating system's message alone.
        io::Outcome::Lost(error) => unfinished(io::NotRun::Spawn(error).to_string()),
    }
}

fn exited(
    subcommand: &'static str,
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: &[u8],
) -> GitOutcome {
    let code = io::exit_code(status);
    if code == 0 {
        return GitOutcome::Printed(stdout);
    }
    let detail = first_line(stderr);
    GitOutcome::Failed(Failure {
        subcommand,
        code,
        detail,
    })
}

fn first_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines = text::splitlines(text::strip(&text), false);
    lines
        .first()
        .map_or_else(|| "no detail".to_owned(), |line| (*line).to_owned())
}

#[cfg(test)]
mod tests;
