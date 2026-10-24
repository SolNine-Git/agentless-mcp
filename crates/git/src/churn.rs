//! Commit activity per file: how many commits inside the window touched it, and the newest one.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use crate::snapshot::RECEIPT;
use crate::{Git, GitOutcome};

/// The window that churn counts commits in, in days before now.
pub const CHURN_WINDOW_DAYS: u32 = 90;

// The pretty format opens each commit with a NUL, because --name-only prints bare file names and a
// file name can be all digits. No file name holds a NUL.
const PRETTY: &str = "--pretty=%x00%ct";

/// One file's commits inside the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Churn {
    /// The number of commits inside the window that touched the file.
    pub commits: usize,
    /// The commit time of the newest of them in Unix seconds, or `None` when there is none.
    pub last_commit_ts: Option<i64>,
}

impl Git {
    /// Counts each of `paths`' commits inside the window with one bounded git log. Returns `None`
    /// when git cannot answer, and runs no git for an empty `paths`.
    #[must_use]
    pub fn churn(&self, root: &Path, paths: &[&str]) -> Option<BTreeMap<String, Churn>> {
        if paths.is_empty() {
            return Some(BTreeMap::new());
        }
        let since = OsString::from(format!("--since={CHURN_WINDOW_DAYS}.days"));
        // "./" defeats pathspec magic, so a file named ":!x.py" is a path and not an exclusion.
        let targets: Vec<OsString> = paths
            .iter()
            .map(|path| OsString::from(format!("./{path}")))
            .collect();
        let fixed = [PRETTY, "--name-only", "--relative", "--"].map(OsStr::new);
        let rest: Vec<&OsStr> = std::iter::once(since.as_os_str())
            .chain(fixed)
            .chain(targets.iter().map(OsString::as_os_str))
            .collect();
        // A capped log is unknown, not partial: half a log would undercount.
        let GitOutcome::Printed(stdout) = self.run(root, "log", &rest, RECEIPT) else {
            return None;
        };
        let text = String::from_utf8_lossy(text::strip_bytes(&stdout));
        Some(tally(&text, paths))
    }
}

// The log is newest first, so the first stamp seen for a path is its newest.
fn tally(log: &str, paths: &[&str]) -> BTreeMap<String, Churn> {
    let none = Churn {
        commits: 0,
        last_commit_ts: None,
    };
    let mut counts: BTreeMap<String, Churn> = paths
        .iter()
        .map(|path| ((*path).to_owned(), none))
        .collect();
    let mut stamp = None;
    for raw in text::splitlines(log, false) {
        let line = text::strip(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(digits) = line.strip_prefix('\0') {
            stamp = commit_time(digits);
        } else if let (Some(churn), Some(time)) = (counts.get_mut(line), stamp) {
            churn.commits += 1;
            churn.last_commit_ts.get_or_insert(time);
        }
    }
    counts
}

// Git prints %ct in ASCII digits. Anything else reads as no time.
fn commit_time(digits: &str) -> Option<i64> {
    let ascii = !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit());
    ascii.then(|| digits.parse().ok()).flatten()
}

#[cfg(test)]
mod tests;
