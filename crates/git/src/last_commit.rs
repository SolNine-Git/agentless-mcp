//! The newest commit that touched one path: `git log -1` on that path, stopped at the caller's
//! timeout.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;

use crate::history::{FORMAT, MAX_HISTORY_OUTPUT_BYTES, parse_log};
use crate::run::classify;
use crate::toplevel::says_not_a_repository;
use crate::{Commit, Failure, Git, GitOutcome, Limits, Unfinished};

// One commit is far below the cap. The caller's timeout replaces `seconds`.
pub(crate) const LAST_COMMIT: Limits = Limits {
    seconds: 2,
    shown: "2.0",
    cap: MAX_HISTORY_OUTPUT_BYTES,
};

// Git prints this under LC_ALL=C for a log on a branch that has no commit.
const NO_COMMIT_YET: &str = "does not have any commits yet";

/// How the read of the newest commit that touched one path ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LastCommit {
    /// The newest commit that touched the path.
    Found(Commit),
    /// No commit touched the path, or the branch has no commit yet.
    NoCommit,
    /// No repository encloses the root.
    NotRepository,
    /// The timeout passed and git was killed.
    TimedOut,
    /// Git failed another way, or printed output that the format cannot have written. The note
    /// says why.
    Failed(String),
}

impl Git {
    /// Reads the newest commit in HEAD's history that touched `path`, which is relative to `root`,
    /// and stops git when `timeout` passes.
    #[must_use]
    pub fn last_commit(&self, root: &Path, path: &str, timeout: Duration) -> LastCommit {
        // ":(literal)" keeps the path as written: no pathspec magic and no glob widens it.
        let target = OsString::from(format!(":(literal){path}"));
        let rest = [
            OsStr::new("--max-count=1"),
            OsStr::new("-z"),
            OsStr::new(FORMAT),
            OsStr::new("--"),
            &target,
        ];
        let mut spec = self.spec(root, "log", &rest, LAST_COMMIT);
        spec.timeout = timeout;
        answer(classify("log", LAST_COMMIT, io::run(&spec)))
    }
}

fn answer(outcome: GitOutcome) -> LastCommit {
    match outcome {
        GitOutcome::Printed(stdout) => printed(&String::from_utf8_lossy(&stdout)),
        GitOutcome::Capped(_) => LastCommit::Failed(format!(
            "git log printed more than {MAX_HISTORY_OUTPUT_BYTES} bytes for one commit"
        )),
        GitOutcome::Failed(failure) => failed(&failure),
        GitOutcome::Unfinished(Unfinished::TimedOut { .. }) => LastCommit::TimedOut,
        GitOutcome::Unfinished(unfinished) => LastCommit::Failed(unfinished.to_string()),
    }
}

// The log lists at most one commit, so an empty log means that no commit touched the path.
fn printed(text: &str) -> LastCommit {
    match parse_log(text, false) {
        Ok(commits) => (commits.into_iter().next()).map_or(LastCommit::NoCommit, LastCommit::Found),
        Err(defect) => LastCommit::Failed(defect),
    }
}

fn failed(failure: &Failure) -> LastCommit {
    if says_not_a_repository(failure.detail.as_bytes()) {
        LastCommit::NotRepository
    } else if failure.detail.contains(NO_COMMIT_YET) {
        LastCommit::NoCommit
    } else {
        LastCommit::Failed(failure.to_string())
    }
}

#[cfg(test)]
mod tests;
