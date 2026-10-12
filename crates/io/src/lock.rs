//! An exclusive lock file that refuses at once instead of waiting.

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

use crate::strerror;

/// An exclusive lock on a file. The drop closes the file, and the close releases the lock.
#[derive(Debug)]
pub struct LockGuard {
    // Held only so that the lock lives as long as the guard.
    _file: File,
}

/// Why [`try_lock`] holds no lock. It displays as the reason.
#[derive(Debug)]
pub enum LockError {
    /// The lock file could not be opened or created.
    Open(io::Error),
    /// Another open file holds the lock.
    Held,
    /// The file system refused the lock, for example a mount that cannot lock.
    Refused(io::Error),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => write!(f, "the lock file cannot be opened: {}", strerror(error)),
            Self::Held => f.write_str("another process holds the lock"),
            Self::Refused(error) => write!(f, "the lock was refused: {}", strerror(error)),
        }
    }
}

impl std::error::Error for LockError {}

/// Takes an exclusive lock on `path` without waiting, and creates the file when it is missing.
/// The file is opened to append, so it is never truncated, and nothing removes it.
///
/// # Errors
///
/// Returns [`LockError::Held`] when the lock is taken, and another variant when the open or the
/// lock fails.
pub fn try_lock(path: &Path) -> Result<LockGuard, LockError> {
    let file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map_err(LockError::Open)?;
    match file.try_lock() {
        Ok(()) => Ok(LockGuard { _file: file }),
        Err(TryLockError::WouldBlock) => Err(LockError::Held),
        Err(TryLockError::Error(error)) => Err(LockError::Refused(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::tree;

    #[test]
    fn a_second_lock_is_refused_until_the_first_drops() {
        let tree = tree();
        let path = tree.join("write.lock");
        let first = try_lock(&path).expect("the first lock is taken");
        assert!(matches!(try_lock(&path), Err(LockError::Held)));
        drop(first);
        assert!(try_lock(&path).is_ok(), "the drop releases the lock");
    }

    #[test]
    fn the_lock_keeps_the_file_and_its_contents() {
        let tree = tree();
        let path = tree
            .write_file("write.lock", "kept")
            .expect("the lock file is written");
        drop(try_lock(&path).expect("the lock is taken"));
        assert_eq!(std::fs::read(&path).expect("the lock file stays"), b"kept");
    }

    #[test]
    fn a_missing_directory_is_an_open_error() {
        let tree = tree();
        let refused = try_lock(&tree.join("missing/write.lock"));
        assert!(matches!(refused, Err(LockError::Open(_))), "{refused:?}");
    }
}
