//! The agentless-mcp binary: one read of the environment, then a hook or the command line.

use std::ffi::OsString;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use std::sync::OnceLock;

use app::{EnvConfig, Services};
use cli::Streams;

static PANIC_LINE: OnceLock<String> = OnceLock::new();

fn main() -> ExitCode {
    // Before any thread starts: a write past the file size limit is then an error, not a death.
    ::io::catch_file_size_signal();
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let env = match std::env::current_dir() {
        Ok(cwd) => EnvConfig::new(std::env::vars_os(), cwd),
        Err(error) => EnvConfig::with_unread_cwd(std::env::vars_os(), &error),
    };
    let services = Services::new(&env);
    ExitCode::from(dispatch(&args, &env, &services))
}

// The hooks need no start-up directory, so only the other command lines stop on an unread one.
fn dispatch(args: &[OsString], env: &EnvConfig, services: &Services) -> u8 {
    let words: Vec<Option<&str>> = args.iter().map(|arg| arg.to_str()).collect();
    match words.as_slice() {
        [Some("hook"), Some("erosion")] => return erosion(env, services),
        [Some("hook"), Some("gate-check")] => return searched(env),
        [Some("hook"), Some("gate-mark")] => return marked(env),
        _ => {}
    }
    if words.first() == Some(&Some("gate-report")) {
        return with_streams(|streams| {
            guarded(streams, |streams| gate_report(&args[1..], env, streams))
        });
    }
    if let Err(unread) = env.cwd() {
        let line = cli::cwd_failure_line(&unread.error());
        return with_streams(|streams| cli::write_answer(streams, "", &line, cli::EXIT_DOMAIN));
    }
    match words.as_slice() {
        [Some("hook"), ..] => with_streams(cli::refuse_hook),
        _ => with_streams(|streams| {
            guarded(streams, |streams| {
                cli::run(args, env, services, &mut io::stdin().lock(), streams)
            })
        }),
    }
}

fn erosion(env: &EnvConfig, services: &Services) -> u8 {
    answered(|| hook::erosion(env, services, &mut io::stdin().lock()))
}

// The search gate writes its deny message itself, because a failed write changes the exit code.
fn searched(env: &EnvConfig) -> u8 {
    keep_panic_lines();
    hook::gate_check(env, &mut io::stdin().lock(), &mut io::stderr().lock())
}

fn marked(env: &EnvConfig) -> u8 {
    keep_panic_lines();
    hook::gate_mark(env, &mut io::stdin().lock());
    0
}

// The report opens its paths as given, so like the hook it needs no start-up directory.
fn gate_report(args: &[OsString], env: &EnvConfig, streams: &mut Streams<'_>) -> u8 {
    match cli::gate_report_args(args, env, streams) {
        Ok(request) => {
            let output = hook::gate_report(
                &request.log,
                request.sample_out.as_deref(),
                request.labels.as_deref(),
                &request.seed,
            );
            cli::write_answer(streams, &output.stdout, &output.stderr, output.code)
        }
        Err(code) => code,
    }
}

// The hook catches its own panics and reports them, so the panic hook only keeps their lines.
// The locks come after the hook: its SIGTERM watcher writes stderr on its own thread.
fn answered(gate: impl FnOnce() -> hook::Output) -> u8 {
    keep_panic_lines();
    let output = gate();
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

// A panic in the command line prints one line and exits 1. The line comes from the first panic,
// and the main thread writes it, because it holds the stderr lock.
fn guarded(streams: &mut Streams<'_>, run: impl FnOnce(&mut Streams<'_>) -> u8) -> u8 {
    keep_panic_lines();
    match panic::catch_unwind(AssertUnwindSafe(|| run(streams))) {
        Ok(code) => code,
        Err(_payload) => {
            if let Some(line) = PANIC_LINE.get() {
                // Stderr is the last channel: a failed write there has nowhere to go.
                drop(streams.err.write_all(line.as_bytes()));
            }
            cli::EXIT_DOMAIN
        }
    }
}

// The panic hook does no I/O: a helper thread that writes stderr would wait forever for the lock
// that the main thread holds while it joins that helper.
fn keep_panic_lines() {
    panic::set_hook(Box::new(|info| {
        PANIC_LINE.get_or_init(|| cli::internal_error_line(info));
    }));
}

#[cfg(test)]
mod tests;
