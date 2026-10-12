//! The top of the repository that encloses a directory, or the reason git named none.

use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use crate::run::exit_code;
use crate::{Git, Limits};

// A root path is far shorter than the cap. The caller's timeout replaces `seconds`.
const TOPLEVEL: Limits = Limits {
    seconds: 2,
    shown: "2.0",
    cap: 65_536,
};

// Git prints this under LC_ALL=C when no repository encloses the directory.
const NOT_A_REPOSITORY: &[u8] = b"not a git repository";

/// How `git rev-parse --show-toplevel` ended in one directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Toplevel {
    /// Git exited 0, and this is its whole stdout.
    Found(Vec<u8>),
    /// Git exited with a status other than 0 because no repository encloses the directory.
    NotRepository,
    /// Git exited with a status other than 0 for another reason.
    Failed {
        /// The exit status, or the negative signal number that killed git.
        code: i32,
        /// The stderr bytes, at most 64 KiB.
        stderr: Vec<u8>,
    },
    /// Git printed more than 64 KiB and was killed.
    Capped,
    /// The timeout passed and git was killed.
    TimedOut,
    /// No git program was found.
    NotInstalled,
    /// Git could not be started, or a pipe read or the wait failed after the start.
    NotRun {
        /// The operating system's error number, when the error has one.
        errno: Option<i32>,
        /// The operating system's message.
        message: String,
    },
}

impl Git {
    /// Runs `git rev-parse --show-toplevel` in `dir`, and stops it when `timeout` passes.
    #[must_use]
    pub fn toplevel(&self, dir: &Path, timeout: Duration) -> Toplevel {
        let rest = [OsStr::new("--show-toplevel")];
        let mut spec = self.spec(dir, "rev-parse", &rest, TOPLEVEL);
        spec.timeout = timeout;
        classify(io::run(&spec))
    }
}

fn classify(outcome: io::Outcome) -> Toplevel {
    match outcome {
        io::Outcome::Exited {
            status,
            stdout,
            stderr,
        } => exited(exit_code(status), stdout, stderr),
        io::Outcome::Truncated { .. } => Toplevel::Capped,
        io::Outcome::TimedOut => Toplevel::TimedOut,
        io::Outcome::NotRun(io::NotRun::ProgramNotFound(_)) => Toplevel::NotInstalled,
        io::Outcome::NotRun(io::NotRun::Spawn(error)) | io::Outcome::Lost(error) => {
            Toplevel::NotRun {
                errno: error.raw_os_error(),
                message: io::strerror(&error),
            }
        }
    }
}

fn exited(code: i32, stdout: Vec<u8>, stderr: Vec<u8>) -> Toplevel {
    if code == 0 {
        return Toplevel::Found(stdout);
    }
    if stderr
        .windows(NOT_A_REPOSITORY.len())
        .any(|window| window == NOT_A_REPOSITORY)
    {
        return Toplevel::NotRepository;
    }
    Toplevel::Failed { code, stderr }
}

#[cfg(test)]
mod tests;
