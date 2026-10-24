//! A one-write line append, for a log that several processes can share.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::strerror;

/// Why [`append_line`] did not append the whole line.
#[derive(Debug)]
pub enum AppendError {
    /// Creating a parent directory, opening the file, or the write failed.
    Io(io::Error),
    /// The one write took fewer bytes than the line holds.
    Short {
        /// The bytes written.
        written: usize,
        /// The bytes in the line.
        expected: usize,
    },
}

impl fmt::Display for AppendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => f.write_str(&strerror(error)),
            Self::Short { written, expected } => write!(f, "wrote {written} of {expected} bytes"),
        }
    }
}

impl std::error::Error for AppendError {}

/// Appends `line` to `path` in one write. It creates the missing parent directories, and it
/// creates the file with mode 0600 before the umask.
///
/// # Errors
///
/// Returns an [`AppendError`] when a step fails or the write is short.
pub fn append_line(path: &Path, line: &[u8]) -> Result<(), AppendError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(AppendError::Io)?;
    }
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)
        .map_err(AppendError::Io)?;
    let written = write_once(&mut file, line).map_err(AppendError::Io)?;
    if written == line.len() {
        Ok(())
    } else {
        Err(AppendError::Short {
            written,
            expected: line.len(),
        })
    }
}

// One write call, so that appends from two processes do not interleave within a line.
fn write_once(file: &mut File, line: &[u8]) -> io::Result<usize> {
    loop {
        match file.write(line) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            written => return written,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::tree;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn appends_create_the_parents_and_keep_earlier_lines() {
        let tree = tree();
        let path = tree.join("a/b/log.jsonl");
        append_line(&path, b"one\n").expect("the first append succeeds");
        append_line(&path, b"two\n").expect("the second append succeeds");
        assert_eq!(fs::read(&path).expect("the log exists"), b"one\ntwo\n");
        let mode = fs::metadata(&path)
            .expect("the log exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "group and others get no access: {mode:o}");
    }

    #[test]
    fn a_failure_returns_the_error() {
        let tree = tree();
        let blocker = tree.join("file");
        append_line(&blocker, b"x\n").expect("the plain file is created");
        let refused = append_line(&blocker.join("log"), b"x\n");
        assert!(matches!(refused, Err(AppendError::Io(_))), "{refused:?}");
        let shown = refused.map_err(|error| error.to_string());
        assert_eq!(shown, Err("File exists".to_owned()), "the OS message alone");
    }

    #[test]
    fn a_short_write_displays_its_counts() {
        let short = AppendError::Short {
            written: 3,
            expected: 5,
        };
        assert_eq!(short.to_string(), "wrote 3 of 5 bytes");
    }
}
