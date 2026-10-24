//! The server log on standard error: one line for each warning or unplanned failure.

use std::io::Write;

use crate::server::Log;

pub(crate) struct StderrLog;

impl Log for StderrLog {
    fn line(&self, text: &str) {
        // Standard error is the last channel: a failed write there has nowhere to go.
        drop(writeln!(std::io::stderr().lock(), "{text}"));
    }
}
