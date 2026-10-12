//! The agentless-mcp binary: one read of the environment, then the erosion hook or the command
//! line.

use std::ffi::OsString;
use std::io::{self, Write};
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;

use app::{EnvConfig, Services};
use cli::Streams;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let env = match std::env::current_dir() {
        Ok(cwd) => EnvConfig::new(std::env::vars_os(), cwd),
        Err(error) => EnvConfig::with_unread_cwd(std::env::vars_os(), &error),
    };
    let services = Services::new(&env);
    ExitCode::from(dispatch(&args, &env, &services))
}

// The hook needs no start-up directory, so only the other command lines stop on an unread one.
fn dispatch(args: &[OsString], env: &EnvConfig, services: &Services) -> u8 {
    let words: Vec<Option<&str>> = args.iter().map(|arg| arg.to_str()).collect();
    if words.as_slice() == [Some("hook"), Some("erosion")] {
        return erosion(env, services);
    }
    if let Err(unread) = env.cwd() {
        let line = cli::cwd_failure_line(&unread.error());
        return with_streams(|streams| cli::write_answer(streams, "", &line, cli::EXIT_DOMAIN));
    }
    match words.as_slice() {
        [Some("hook"), ..] => with_streams(cli::refuse_hook),
        _ => with_streams(|streams| guarded(args, env, services, streams)),
    }
}

fn erosion(env: &EnvConfig, services: &Services) -> u8 {
    // The locks come after the hook: its SIGTERM watcher writes stderr on its own thread.
    let output = hook::erosion(env, services, &mut io::stdin().lock());
    with_streams(|s| cli::write_answer(s, &output.stdout, &output.stderr, output.code))
}

// Runs `write` with stdout and stderr locked, and returns its exit code.
fn with_streams(write: impl FnOnce(&mut Streams<'_>) -> u8) -> u8 {
    let (stdout, stderr) = (io::stdout(), io::stderr());
    let (mut out, mut err) = (stdout.lock(), stderr.lock());
    write(&mut Streams {
        out: &mut out,
        err: &mut err,
    })
}

// A panic in the command line prints one line and exits 1. The hook catches its own panics.
fn guarded(
    args: &[OsString],
    env: &EnvConfig,
    services: &Services,
    streams: &mut Streams<'_>,
) -> u8 {
    panic::set_hook(Box::new(|info| {
        let line = cli::internal_error_line(info);
        // Stderr is the last channel: a failed write there has nowhere to go.
        drop(io::stderr().write_all(line.as_bytes()));
    }));
    let run = AssertUnwindSafe(|| cli::run(args, env, services, streams));
    panic::catch_unwind(run).unwrap_or(cli::EXIT_DOMAIN)
}
