//! The repository that holds a directory: its git top level, resolved, or why git named none.

use std::path::{Path, PathBuf};
use std::time::Duration;

use app::Services;
use git::{Git, Toplevel};

use crate::text::{codec_text, last_line};

// The erosion gate's git timeout.
const GIT_TIMEOUT: Duration = Duration::from_secs(2);

// Why a directory has no repository root.
#[derive(Debug)]
pub(crate) enum NoRoot {
    // No repository holds the directory.
    Outside,
    // Git printed no top level: why, in the words of the erosion record.
    Git(String),
    // The printed top level is not UTF-8: the decode error's text.
    NotUtf8(String),
    // The printed top level does not resolve.
    Unresolved(std::io::Error),
}

impl NoRoot {
    // The error that a hook logs: none for a directory outside every repository.
    pub(crate) fn cause(&self) -> Option<String> {
        match self {
            NoRoot::Outside => None,
            NoRoot::Git(text) | NoRoot::NotUtf8(text) => Some(text.clone()),
            NoRoot::Unresolved(error) => Some(io::os_error_str(error)),
        }
    }
}

pub(crate) fn root_of(services: &Services, dir: &Path) -> Result<PathBuf, NoRoot> {
    top_level(&services.git, dir, GIT_TIMEOUT)
}

pub(crate) fn top_level(git: &Git, dir: &Path, timeout: Duration) -> Result<PathBuf, NoRoot> {
    resolved(&printed(git.toplevel(dir, timeout), timeout)?)
}

// The printed top level, or why git printed none.
pub(crate) fn printed(toplevel: Toplevel, timeout: Duration) -> Result<Vec<u8>, NoRoot> {
    let reason = match toplevel {
        Toplevel::Found(stdout) => return Ok(stdout),
        Toplevel::NotRepository => return Err(NoRoot::Outside),
        Toplevel::Failed { code, stderr } => {
            let last = last_line(&String::from_utf8_lossy(&stderr));
            format!("git rev-parse exited {code}: {last}")
        }
        Toplevel::Capped => "git rev-parse printed more than 65536 bytes".to_owned(),
        Toplevel::TimedOut => {
            let seconds = timeout.as_secs_f64();
            format!("git rev-parse timed out after {seconds:.1} s")
        }
        Toplevel::NotInstalled => not_run(Some(2), "No such file or directory"),
        Toplevel::NotRun { errno, message } => not_run(errno, &message),
    };
    Err(NoRoot::Git(reason))
}

fn not_run(errno: Option<i32>, message: &str) -> String {
    let error = match errno {
        Some(code) => format!("[Errno {code}] {message}: 'git'"),
        None => message.to_owned(),
    };
    format!("cannot run git: {error}")
}

pub(crate) fn resolved(stdout: &[u8]) -> Result<PathBuf, NoRoot> {
    let text = io::decode_strict(stdout.to_vec())
        .map_err(|error| NoRoot::NotUtf8(codec_text(stdout, error)))?;
    io::resolve(Path::new(text::strip(&text))).map_err(NoRoot::Unresolved)
}
