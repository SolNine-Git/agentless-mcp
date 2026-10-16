//! The command line: the parser, the steps of `lint`, `capabilities`, the read subcommands and the
//! write subcommands, and the exit codes.

mod args;
mod caller;
mod commands;
mod exit;
mod invocation;
mod lint;
mod patches;
mod read;
mod steps;
mod validate;
mod vote;

use std::ffi::OsString;
use std::io::{Read, Write};

pub use exit::{EXIT_DOMAIN, cwd_failure_line, internal_error_line, refuse_hook, write_answer};

/// The two output streams of one run: the answer on `out`, and notes and failures on `err`.
pub struct Streams<'a> {
    /// Standard output.
    pub out: &'a mut dyn Write,
    /// Standard error.
    pub err: &'a mut dyn Write,
}

/// Runs one command line, given without the program name, and returns its exit code. A patch
/// command without a patch file reads its patch from `stdin`.
#[must_use]
pub fn run(
    args: &[OsString],
    env: &app::EnvConfig,
    services: &app::Services,
    stdin: &mut dyn Read,
    streams: &mut Streams<'_>,
) -> u8 {
    steps::run(args, env, services, stdin, streams)
}
