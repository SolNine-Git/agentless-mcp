//! The command line: the parser, the steps of `lint`, `capabilities` and the read subcommands,
//! and the exit codes.

mod args;
mod commands;
mod exit;
mod invocation;
mod lint;
mod read;
mod steps;

use std::ffi::OsString;
use std::io::Write;

pub use exit::{EXIT_DOMAIN, cwd_failure_line, internal_error_line, refuse_hook, write_answer};

/// The two output streams of one run: the answer on `out`, and notes and failures on `err`.
pub struct Streams<'a> {
    /// Standard output.
    pub out: &'a mut dyn Write,
    /// Standard error.
    pub err: &'a mut dyn Write,
}

/// Runs one command line, given without the program name, and returns its exit code.
#[must_use]
pub fn run(
    args: &[OsString],
    env: &app::EnvConfig,
    services: &app::Services,
    streams: &mut Streams<'_>,
) -> u8 {
    steps::run(args, env, services, streams)
}
