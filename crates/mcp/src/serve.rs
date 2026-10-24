//! The start of a server: its transport, its background index and its restart monitor, from the
//! command line and the environment, then the serve until the transport ends.

use std::ffi::OsStr;
use std::net::SocketAddr;
use std::sync::Arc;

use app::{EnvConfig, Services};

use crate::autoindex::{AutoIndex, Disarmed};
use crate::log::StderrLog;
use crate::restart::{Installed, Monitor, Relaunch};
use crate::server::{Arming, Log, Ports, Server, ServerConfig};
use crate::{http, stdio};

const NO_AUTO_INDEX: &str = "AGENTLESS_MCP_NO_AUTO_INDEX";
const NO_AUTO_RESTART: &str = "AGENTLESS_MCP_NO_AUTO_RESTART";
// The values of an opt-out variable that keep the feature on.
const KEPT: [&str; 4] = ["", "0", "false", "False"];

/// What the server door starts a server with.
pub struct Start {
    /// The roots, the limits and the launch directory.
    pub config: ServerConfig,
    /// How the server talks to its client.
    pub transport: Transport,
    /// `--no-auto-index`.
    pub no_auto_index: bool,
    /// `--no-auto-restart`.
    pub no_auto_restart: bool,
    /// The installed binary that a restart runs, or `None` when its path is unknown.
    pub relaunch: Option<Relaunch>,
}

/// How a server talks to its client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Standard input and output.
    Stdio,
    /// Streamable HTTP on this loopback address.
    Http(SocketAddr),
}

/// How a serve ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Ended {
    /// The client ended it.
    Closed,
    /// This signal stopped it after the drain; the door ends the process by the same signal.
    Signalled(i32),
    /// It could not start or go on; the text says why.
    Failed(String),
    /// A write to the client failed, so the session ended; the text says why.
    Unwritable(String),
    /// The installed binary changed, so the door runs [`Start::relaunch`] in this process.
    Relaunch,
}

/// Serves `start` until its transport ends, and says how it ended.
#[must_use]
pub fn serve(start: Start, env: &EnvConfig, services: Arc<Services>) -> Ended {
    let built = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let runtime = match built {
        Ok(runtime) => runtime,
        Err(error) => {
            let error = io::os_error_line(&error);
            return Ended::Failed(format!(
                "agentless-mcp-server: cannot start the runtime: {error}"
            ));
        }
    };
    let log: Arc<dyn Log> = Arc::new(StderrLog);
    let arming = arming(&start, env, &services, &log);
    let monitor = watched(&start, env)
        .then(|| monitor(&start, &services))
        .flatten();
    let ports = Ports {
        arming,
        log: Arc::clone(&log),
    };
    let server = Server::new(start.config, services, ports);
    match start.transport {
        Transport::Stdio => runtime.block_on(stdio::serve(server, rmcp::transport::stdio())),
        Transport::Http(address) => runtime.block_on(http::serve(server, address, monitor, log)),
    }
}

fn arming(
    start: &Start,
    env: &EnvConfig,
    services: &Arc<Services>,
    log: &Arc<dyn Log>,
) -> Arc<dyn Arming> {
    if !indexed(start, env) {
        return Arc::new(Disarmed);
    }
    let ceiling = env
        .var(store::MAX_CACHE_BYTES_VARIABLE)
        .map(OsStr::to_os_string);
    Arc::new(AutoIndex::new(
        Arc::clone(services),
        ceiling,
        Arc::clone(log),
    ))
}

pub(crate) fn indexed(start: &Start, env: &EnvConfig) -> bool {
    !start.no_auto_index && !opted_out(env, NO_AUTO_INDEX)
}

// Only the long-running transport can drift from its install: a stdio process loads the new binary
// when its client starts the next one.
pub(crate) fn watched(start: &Start, env: &EnvConfig) -> bool {
    matches!(start.transport, Transport::Http(_))
        && !start.no_auto_restart
        && !opted_out(env, NO_AUTO_RESTART)
}

fn monitor(start: &Start, services: &Services) -> Option<Monitor> {
    let relaunch = start.relaunch.as_ref()?;
    let clock = Arc::clone(&services.clock);
    Some(Monitor {
        source: Arc::new(Installed(relaunch.program.clone())),
        started: clock.now(),
        clock,
    })
}

// A variable opts out unless it is unset or holds one of the values that keep the feature on.
pub(crate) fn opted_out(env: &EnvConfig, name: &str) -> bool {
    env.var(name)
        .is_some_and(|value| !KEPT.iter().any(|kept| value == OsStr::new(kept)))
}
