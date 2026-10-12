//! The start-up configuration, read once, and the services every operation shares.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The time the clone index may spend on the repository walk and the files it reads.
pub const WALK_BUDGET: Duration = Duration::from_secs(10);

/// The process environment and working directory, read once at start-up.
#[derive(Clone, Debug)]
pub struct EnvConfig {
    vars: Vec<(OsString, OsString)>,
    cwd: Result<PathBuf, UnreadCwd>,
}

/// The error that kept the start-up working directory unread, for example a deleted directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnreadCwd {
    code: Option<i32>,
    kind: ErrorKind,
    message: String,
}

impl UnreadCwd {
    /// Returns the operating system's error.
    #[must_use]
    pub fn error(&self) -> std::io::Error {
        match self.code {
            Some(code) => std::io::Error::from_raw_os_error(code),
            None => std::io::Error::new(self.kind, self.message.clone()),
        }
    }
}

impl fmt::Display for UnreadCwd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = io::strerror(&self.error());
        write!(f, "the start-up directory is unreadable: {reason}")
    }
}

impl EnvConfig {
    /// Keeps `vars` and `cwd` as the start-up environment and working directory. A repeated name
    /// keeps its first value.
    pub fn new(vars: impl IntoIterator<Item = (OsString, OsString)>, cwd: PathBuf) -> Self {
        Self {
            vars: first_of_each(vars),
            cwd: Ok(cwd),
        }
    }

    /// Keeps `vars` as the start-up environment, and `error` as why the working directory is
    /// unknown. A repeated name keeps its first value.
    pub fn with_unread_cwd(
        vars: impl IntoIterator<Item = (OsString, OsString)>,
        error: &std::io::Error,
    ) -> Self {
        let unread = UnreadCwd {
            code: error.raw_os_error(),
            kind: error.kind(),
            message: error.to_string(),
        };
        Self {
            vars: first_of_each(vars),
            cwd: Err(unread),
        }
    }

    /// Returns the value of the variable `name`, or `None` when it is unset.
    #[must_use]
    pub fn var(&self, name: &str) -> Option<&OsStr> {
        self.vars
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_os_str())
    }

    /// Returns every variable, in start-up order, with one entry for each name.
    #[must_use]
    pub fn vars(&self) -> &[(OsString, OsString)] {
        &self.vars
    }

    /// Returns the start-up working directory, or why it is unknown.
    ///
    /// # Errors
    ///
    /// Returns the [`UnreadCwd`] when the directory could not be read at start-up.
    pub fn cwd(&self) -> Result<&Path, &UnreadCwd> {
        self.cwd.as_deref()
    }
}

// One value for each name, so `var`, the git child and its PATH search all read the same entry.
fn first_of_each(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    let mut seen = HashSet::new();
    vars.into_iter()
        .filter(|(name, _)| seen.insert(name.clone()))
        .collect()
}

/// The ports and settings that every operation reads.
#[derive(Clone)]
pub struct Services {
    /// The git runner, with the start-up environment.
    pub git: git::Git,
    /// The start-up working directory, which a relative path argument names a path below, or why
    /// it is unknown.
    pub cwd: Result<PathBuf, UnreadCwd>,
    /// The start-up `HOME` value, which expands a leading `~` in `--repo`.
    pub home: Option<OsString>,
    /// The cache root, or why the home directory that it needs is unknown.
    pub cache_root: Result<io::CacheRoot, io::HomeUnknown>,
    /// The time the clone index may spend.
    pub walk_budget: Duration,
}

impl Services {
    /// Builds the services from the start-up environment.
    #[must_use]
    pub fn new(env: &EnvConfig) -> Self {
        Self {
            git: git::Git::new(env.vars().iter().cloned()),
            cwd: env.cwd.clone(),
            home: env.var("HOME").map(OsStr::to_os_string),
            cache_root: io::cache_root(|name| env.var(name).map(OsStr::to_os_string)),
            walk_budget: WALK_BUDGET,
        }
    }
}

#[cfg(test)]
mod tests;
