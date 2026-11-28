//! Whether the index holds a path: one bounded `ls-files` of it.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;

use crate::{Failure, Git, GitOutcome, Limits, Unfinished};

// The listing names the path and the entries below it. A git killed at the cap has listed one.
const TRACKED: Limits = Limits {
    seconds: 5,
    shown: "5.0",
    cap: 65_536,
};

/// Why [`Git::tracks`] gave no answer. It displays as the note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackedError {
    /// Git exited with a status other than 0, for example outside a repository.
    Failed(Failure),
    /// Git gave no exit status: it timed out, did not start, or was lost.
    Unfinished(Unfinished),
}

impl Git {
    /// Says whether the index holds `path` under `root`, or an entry below it, in any letter case.
    ///
    /// # Errors
    ///
    /// Returns the [`TrackedError`] of a git that listed nothing to answer with.
    pub fn tracks(&self, root: &Path, path: &Path) -> Result<bool, TrackedError> {
        // "icase" also matches the entry that a case-insensitive file system checks out at `path`.
        let mut target = OsString::from(":(literal,icase)");
        target.push(path);
        let rest = [
            OsStr::new("--cached"),
            OsStr::new("-z"),
            OsStr::new("--"),
            &target,
        ];
        match self.run(root, "ls-files", &rest, TRACKED) {
            GitOutcome::Printed(stdout) => Ok(!stdout.is_empty()),
            GitOutcome::Capped(_) => Ok(true),
            GitOutcome::Failed(failure) => Err(TrackedError::Failed(failure)),
            GitOutcome::Unfinished(unfinished) => Err(TrackedError::Unfinished(unfinished)),
        }
    }
}

impl fmt::Display for TrackedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(failure) => failure.fmt(f),
            Self::Unfinished(unfinished) => unfinished.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests;
