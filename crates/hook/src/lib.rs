//! The erosion gate that Claude Code runs before each Write and Edit: it lints the proposed text
//! in process, decides whether to block, advise or allow, and appends one log record.

mod call;
mod destination;
mod edit;
mod gate;
mod halt;
mod modes;
mod payload;
mod record;
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

use crate::call::{Call, SystemClock};
use crate::gate::Ports;
use crate::text::panic_text;

const ENV_GATE: &str = "AGENTLESS_EROSION_GATE";
const ENV_LOG: &str = "AGENTLESS_EROSION_LOG";
const OFF: &str = "off";
const CONFIG_FILENAME: &str = io::CONFIG_FILENAME;

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
    if env.var(ENV_GATE) == Some(OsStr::new(OFF)) {
        return Output::default();
    }
    // Fail open: no panic leaves this function.
    panic::catch_unwind(AssertUnwindSafe(|| gated(env, services, stdin))).unwrap_or_else(
        |payload| {
            let message = panic_text(&*payload);
            Output {
                stderr: format!("erosion gate internal error: panic: {message}\n"),
                ..Output::default()
            }
        },
    )
}

fn gated(env: &EnvConfig, services: &Services, stdin: &mut dyn Read) -> Output {
    let log = env.var(ENV_LOG).map(OsStr::to_os_string);
    let call = Arc::new(Call::new(Box::new(SystemClock), log));
    let watch = signals::watch(Arc::clone(&call));
    let ports = Ports {
        services,
        env_gate: env.var(ENV_GATE),
        lint: worker::in_process,
        binary: binary(),
    };
    let mut output = gate::run(&call, &ports, stdin);
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
