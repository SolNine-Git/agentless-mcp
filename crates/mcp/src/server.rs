//! The server: its configuration, the ports that the transports fill, and the state that every
//! call of one server shares.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use app::{Services, Session};
use tokio::sync::Semaphore;

use crate::dispatch::Inner;
use crate::listing::listed_tools;
use crate::offload::{Offload, Threads};
use crate::roots::{Allowlist, RootsFile};

/// What a server is started with.
pub struct ServerConfig {
    /// The `--root` flags, resolved.
    pub roots: Vec<PathBuf>,
    /// The `--roots-from` files.
    pub roots_files: Vec<RootsFile>,
    /// The resolved working directory of a stdio server, or `None` for a shared HTTP server.
    pub launch_dir: Option<PathBuf>,
    /// How many calls may work at once.
    pub max_concurrency: usize,
    /// The seconds of work a call may do.
    pub call_limit: f64,
    /// The most seconds of uncached parsing that a call may add to its limit.
    pub recache_grace: f64,
}

/// The background index: armed after a call opens its repository, before the operation runs.
pub trait Arming: Send + Sync {
    /// Starts the refresh of `session`'s cache when it needs one.
    ///
    /// # Errors
    ///
    /// Returns the reason when the operating system refuses the refresh's thread.
    fn arm(&self, session: &Session) -> Result<(), String>;

    /// Returns true while a refresh of the cache at `database` runs.
    fn refreshing(&self, database: &Path) -> bool;
}

/// The server log: a warning of an operation, or the detail of a failure the server did not plan
/// for.
pub trait Log: Send + Sync {
    /// Writes one line.
    fn line(&self, text: &str);
}

/// The ports that the transports fill.
pub struct Ports {
    /// The background index.
    pub arming: Arc<dyn Arming>,
    /// The server log.
    pub log: Arc<dyn Log>,
}

/// The MCP server of the v2 tools. Its clones share one concurrency gate.
#[derive(Clone)]
pub struct Server {
    pub(crate) inner: Arc<Inner>,
    pub(crate) gate: Arc<Semaphore>,
    pub(crate) permits: usize,
    pub(crate) offload: Arc<dyn Offload>,
    pub(crate) tools: Arc<Vec<rmcp::model::Tool>>,
    pub(crate) over_http: bool,
}

impl Server {
    /// Returns the server that `config` describes, on `services` and `ports`.
    #[must_use]
    pub fn new(config: ServerConfig, services: Arc<Services>, ports: Ports) -> Server {
        Server::with_offload(config, services, ports, Arc::new(Threads("mcp-call")))
    }

    // The server whose calls start their work through `offload`.
    pub(crate) fn with_offload(
        config: ServerConfig,
        services: Arc<Services>,
        ports: Ports,
        offload: Arc<dyn Offload>,
    ) -> Server {
        let inner = Inner {
            allowlist: Allowlist::new(config.roots, config.roots_files),
            launch: config.launch_dir,
            call_limit: config.call_limit,
            recache_grace: config.recache_grace,
            services,
            ports,
        };
        Server {
            inner: Arc::new(inner),
            gate: Arc::new(Semaphore::new(config.max_concurrency)),
            permits: config.max_concurrency,
            offload,
            tools: Arc::new(listed_tools()),
            over_http: false,
        }
    }
}
