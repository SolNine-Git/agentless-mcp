//! The discard of a database that this build cannot use, under the index write lock.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use io::{LockError, os_error_class_text};

const LOCK_NAME: &str = "write.lock";
// The sidecars go first and the primary last: a sidecar that stays keeps the primary visible to
// the next sweep.
const SUFFIXES: [&str; 3] = ["-shm", "-wal", ""];

/// What the discard of a database did.
#[derive(Debug)]
pub enum Discarded {
    /// The database and its sidecars are gone.
    Removed,
    /// An index run holds the write lock, or the file system cannot lock. Nothing was removed.
    LeftInPlace,
    /// The lock file or a database file could not be changed.
    Failed(DiscardFailure),
}

/// A discard that failed. It displays as the warning line.
#[derive(Debug)]
pub struct DiscardFailure {
    database: PathBuf,
    cause: std::io::Error,
}

impl fmt::Display for DiscardFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "tag cache {} could not be discarded: {}",
            self.database.display(),
            os_error_class_text(&self.cause)
        )
    }
}

/// Removes the database at `database` and its `-wal` and `-shm` sidecars, while it holds the
/// index write lock in the same directory.
#[must_use]
pub fn discard(database: &Path) -> Discarded {
    let failed = |cause| {
        Discarded::Failed(DiscardFailure {
            database: database.to_path_buf(),
            cause,
        })
    };
    let lock = match io::try_lock(&database.with_file_name(LOCK_NAME)) {
        Ok(lock) => lock,
        Err(LockError::Held | LockError::Refused(_)) => return Discarded::LeftInPlace,
        Err(LockError::Open(cause)) => return failed(cause),
    };
    let removed = SUFFIXES
        .iter()
        .try_for_each(|suffix| io::remove_file(&with_suffix(database, suffix)).map(drop));
    drop(lock);
    match removed {
        Ok(()) => Discarded::Removed,
        Err(cause) => failed(cause),
    }
}

fn with_suffix(database: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(database);
    name.push(suffix);
    PathBuf::from(name)
}
