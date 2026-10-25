//! The MCP server: the v2 tool list, the argument validation, the call dispatch, the repository
//! allowlist, the stdio and HTTP transports, the background index, and the restart on upgrade.

mod answer;
mod arguments;
mod autoindex;
mod dispatch;
mod drain;
mod failure;
mod http;
mod listing;
mod log;
mod offload;
mod operation;
mod refusal;
mod restart;
mod roots;
mod serve;
mod server;
mod service;
mod stdio;
#[cfg(test)]
mod tests;
mod validate;

pub use restart::Relaunch;
pub use roots::{RootsFile, RootsFileError};
pub use serve::{Ended, Start, Transport, serve};
pub use server::{Arming, Log, Ports, Server, ServerConfig};
