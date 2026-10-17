//! The hooks that Claude Code runs around tool calls. The erosion gate lints the proposed text of
//! each Write and Edit, decides whether to block, advise or allow, and appends one log record.
//! The search gate holds broad text search until a localizing agentless call, and records that
//! call.

mod call;
mod destination;
mod edit;
mod gate;
mod halt;
mod modes;
mod payload;
mod record;
mod report;
mod search;
mod signals;
mod text;
mod verdict;
mod worker;

#[cfg(test)]
mod testing;

use std::ffi::OsStr;
use std::io::Read;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use app::{EnvConfig, Services};
use kernel::{Clock, SystemClock};

pub use crate::report::gate_report;
pub use crate::search::{gate_check, gate_mark};

use crate::call::Call;
use crate::gate::Ports;
use crate::signals::Watch;
use crate::text::panic_text;

const ENV_GATE: &str = "AGENTLESS_EROSION_GATE";
const ENV_LOG: &str = "AGENTLESS_EROSION_LOG";
const OFF: &str = "off";

/// What the process writes for one call: the exit code, stdout, and stderr.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    /// 2 when the gate blocks the edit, else 0.
    pub code: u8,
    /// The advisory reply, or empty.
    pub stdout: String,
    /// The blocking findings and any diagnostic lines, or empty.
    pub stderr: String,
}

/// Gates the one `PreToolUse` payload that `stdin` holds, and returns what the process writes.
#[must_use]
pub fn erosion(env: &EnvConfig, services: &Services, stdin: &mut dyn Read) -> Output {
    erosion_with(env, services, stdin, Box::new(SystemClock))
}

fn erosion_with(
    env: &EnvConfig,
    services: &Services,
    stdin: &mut dyn Read,
    clock: Box<dyn Clock>,
) -> Output {
    if env.var(ENV_GATE) == Some(OsStr::new(OFF)) {
        return Output::default();
    }
    // Fail open: no panic leaves this function.
    panic::catch_unwind(AssertUnwindSafe(|| gated(env, services, stdin, clock))).unwrap_or_else(
        |payload| {
            let message = panic_text(&*payload);
            Output {
                stderr: format!("erosion gate internal error: panic: {message}\n"),
                ..Output::default()
            }
        },
    )
}

fn gated(
    env: &EnvConfig,
    services: &Services,
    stdin: &mut dyn Read,
    clock: Box<dyn Clock>,
) -> Output {
    let log = env.var(ENV_LOG).map(OsStr::to_os_string);
    let call = Arc::new(Call::new(clock, log));
    let watch = signals::watch(Arc::clone(&call));
    let ports = Ports {
        services,
        env_gate: env.var(ENV_GATE),
        lint: worker::in_process,
        binary: binary(),
    };
    watched(gate::run(&call, &ports, stdin), watch)
}

// Stops the SIGTERM watch, or puts the note of its failure first on stderr.
fn watched(mut output: Output, watch: std::io::Result<Watch>) -> Output {
    match watch {
        Ok(watch) => watch.stop(),
        Err(error) => {
            let note = format!("erosion gate cannot watch SIGTERM: {error}\n");
            output.stderr.insert_str(0, &note);
        }
    }
    output
}

// The current executable, for the record. A process whose path is unknown logs null.
fn binary() -> Option<String> {
    std::env::current_exe().ok().map(|path| text::shown(&path))
}

#[cfg(test)]
mod tests;
