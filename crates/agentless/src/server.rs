//! The server door: the agentless-mcp-server command line, then the MCP server until its transport
//! ends, then the exec of a restart.

use std::ffi::{OsStr, OsString};
use std::net::SocketAddr;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use app::{EnvConfig, Services};
use cli::{ServerArgs, ServerTransport};
use mcp::{Ended, Relaunch, RootsFile, ServerConfig, Start, Transport};

use crate::with_streams;

// The program name that the thin agentless-mcp-server binary gives this binary.
const SERVER: &str = "agentless-mcp-server";

// The door serves when this binary runs under the server's name: the thin binary sets it, and a
// link of that name gives it too.
pub(crate) fn invoked(program: &OsStr) -> bool {
    Path::new(program).file_name() == Some(OsStr::new(SERVER))
}

pub(crate) fn run(program: OsString, args: &[OsString], env: &EnvConfig, services: Services) -> u8 {
    let services = Arc::new(services);
    let start = match parsed(args, env, &services).map(|parsed| started(parsed, program, args, env))
    {
        Ok(Ok(start)) => start,
        Ok(Err(line)) => return failed(&line),
        Err(code) => return code,
    };
    let relaunch = start.relaunch.clone();
    // A panic on this thread exits 1, after the default hook has printed it.
    match panic::catch_unwind(AssertUnwindSafe(|| mcp::serve(start, env, services))) {
        Ok(Ended::Closed) => 0,
        Ok(Ended::Signalled(signal)) => ended_by(signal),
        Ok(Ended::Failed(line)) => failed(&line),
        Ok(Ended::Relaunch) => relaunch.map_or(0, |relaunch| relaunched(&relaunch)),
        Err(_panic) => cli::EXIT_DOMAIN,
    }
}

// After its drain the server ends by the signal that stopped it, as the reference re-raises it, so
// a supervisor sees death by that signal. The emulation returns only when it fails.
fn ended_by(signal: i32) -> u8 {
    drop(signal_hook::low_level::emulate_default_handler(signal));
    u8::try_from(128 + signal).unwrap_or(cli::EXIT_DOMAIN)
}

// The standard streams are locked for the parse only: the server's threads write the log later.
fn parsed(
    args: &[OsString],
    env: &EnvConfig,
    services: &Services,
) -> Result<ServerArgs<RootsFile>, u8> {
    let (stdout, stderr) = (std::io::stdout(), std::io::stderr());
    let (mut out, mut err) = (stdout.lock(), stderr.lock());
    let mut streams = cli::Streams {
        out: &mut out,
        err: &mut err,
    };
    cli::server_args(args, env, &mut streams, &mut |raw| {
        RootsFile::read(raw, services).map_err(|error| error.to_string())
    })
}

// The server's start from its parsed command line. Under stdio the start-up directory is the launch
// directory; a shared HTTP server has none.
fn started(
    parsed: ServerArgs<RootsFile>,
    program: OsString,
    args: &[OsString],
    env: &EnvConfig,
) -> Result<Start, String> {
    let (transport, launch_dir) = match parsed.transport {
        ServerTransport::Stdio => (Transport::Stdio, Some(launch_dir(env)?)),
        ServerTransport::Http { host, port } => {
            (Transport::Http(SocketAddr::new(host, port)), None)
        }
    };
    let config = ServerConfig {
        roots: parsed.roots,
        roots_files: parsed.roots_from,
        launch_dir,
        max_concurrency: parsed.max_concurrency,
        call_limit: parsed.call_limit,
        recache_grace: parsed.recache_grace,
    };
    Ok(Start {
        config,
        transport,
        no_auto_index: parsed.no_auto_index,
        no_auto_restart: parsed.no_auto_restart,
        relaunch: installed(program, args),
    })
}

fn launch_dir(env: &EnvConfig) -> Result<PathBuf, String> {
    let cwd = env
        .cwd()
        .map_err(|unread| cli::cwd_failure_line(&unread.error()))?;
    io::resolve(cwd).map_err(|error| cli::cwd_failure_line(&error))
}

// The installed binary's path, read once now: after an upgrade replaces the file, this process's
// own link names a deleted file.
fn installed(program: OsString, args: &[OsString]) -> Option<Relaunch> {
    let path = std::env::current_exe().ok()?;
    Some(Relaunch {
        program: path,
        argv0: program,
        args: args.to_vec(),
    })
}

// The exec returns only when it fails; then the process ends with 0, for a supervisor to restart.
fn relaunched(relaunch: &Relaunch) -> u8 {
    failed(&relaunch.exec());
    0
}

fn failed(line: &str) -> u8 {
    let line = format!("{}\n", line.trim_end_matches('\n'));
    with_streams(|streams| cli::write_answer(streams, "", &line, cli::EXIT_DOMAIN))
}

#[cfg(test)]
mod tests;
