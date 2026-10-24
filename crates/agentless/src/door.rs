//! The agentless-mcp-server command: it runs its sibling agentless-mcp binary as the MCP server, so
//! that one install holds one copy of the server.

use std::env;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

// The program name that selects the server door of agentless-mcp.
const SERVER: &str = "agentless-mcp-server";
const SIBLING: &str = "agentless-mcp";

fn main() -> ExitCode {
    let failure = match env::current_exe() {
        Ok(own) => {
            let sibling = own.with_file_name(SIBLING);
            let error = Command::new(&sibling)
                .arg0(SERVER)
                .args(env::args_os().skip(1))
                .exec();
            format!("{SERVER}: cannot run {}: {error}", sibling.display())
        }
        Err(error) => format!("{SERVER}: cannot find its own path: {error}"),
    };
    // Standard error is the last channel: a failed write there has nowhere to go.
    drop(writeln!(std::io::stderr(), "{failure}"));
    ExitCode::FAILURE
}
