//! The MCP server: the v2 tool list, the argument validation, the call dispatch, and the
//! repository allowlist.

mod answer;
mod arguments;
mod dispatch;
mod failure;
mod listing;
mod offload;
mod operation;
mod roots;
mod server;
mod service;
#[cfg(test)]
mod tests;
mod validate;

pub use roots::{RootsFile, RootsFileError};
pub use server::{Arming, Log, Ports, Server, ServerConfig};
