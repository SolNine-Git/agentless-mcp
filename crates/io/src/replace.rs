//! Atomic replacement of one file: an owner-only parent, a private temporary file beside the
//! target, a flush to disk, then a rename over the target.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::files::make_dir;

const FILE_MODE: u32 = 0o600;
// A name that another writer holds is skipped, and a run of taken names is a refusal.
const MAX_NAME_ATTEMPTS: u64 = 100;

static NEXT_NAME: AtomicU64 = AtomicU64::new(0);

/// Replaces the file `target` with `bytes` in one rename: the parent is created with mode 0700
/// when missing, the new file has mode 0600, and a rename never follows a symlink at `target`.
/// The temporary file is removed when a step fails.
///
/// # Errors
///
/// Returns the error of the step that failed: the parent, the temporary file, the write, the
/// flush, the mode, or the rename.
pub fn replace_file(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        let message = format!("{} names no file in a directory", target.display());
        return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
    };
    make_dir(parent)?;
    let (temporary, file) = temporary_file(parent, name)?;
    let replaced = write_synced(file, bytes)
        .and_then(|()| fs::set_permissions(&temporary, Permissions::from_mode(FILE_MODE)))
        .and_then(|()| fs::rename(&temporary, target));
    // The failed step's error is the one returned; a failed cleanup leaves a hidden file.
    if replaced.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    replaced
}

// A new file named `.<name>.<n>` beside the target, created exclusively.
fn temporary_file(directory: &Path, name: &OsStr) -> io::Result<(PathBuf, File)> {
    let process = std::process::id();
    let mut last = None;
    for _ in 0..MAX_NAME_ATTEMPTS {
        let serial = NEXT_NAME.fetch_add(1, Ordering::Relaxed);
        let mut leaf = OsStr::new(".").to_os_string();
        leaf.push(name);
        leaf.push(format!(".{process:x}{serial:x}"));
        let path = directory.join(leaf);
        let created = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(&path);
        match created {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last = Some(error),
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::from(io::ErrorKind::AlreadyExists)))
}

fn write_synced(mut file: File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::tree;

    fn mode(path: &Path) -> u32 {
        let metadata = fs::metadata(path).unwrap_or_else(|error| panic!("{error}"));
        metadata.permissions().mode() & 0o777
    }

    #[test]
    fn a_new_file_lands_private_in_a_private_parent() {
        let tree = tree();
        let target = tree.join("cache/exports/graph.html");
        replace_file(&target, b"<html>").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(fs::read(&target).ok(), Some(b"<html>".to_vec()));
        assert_eq!(mode(&target), FILE_MODE);
        assert_eq!(mode(&tree.join("cache/exports")), 0o700);
    }

    #[test]
    fn an_existing_file_is_replaced_and_no_temporary_file_stays() {
        let tree = tree();
        let target = tree
            .write_file("graph.html", "old")
            .unwrap_or_else(|error| panic!("{error}"));
        replace_file(&target, b"new").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(fs::read(&target).ok(), Some(b"new".to_vec()));
        let names: Vec<_> = fs::read_dir(tree.path())
            .unwrap_or_else(|error| panic!("{error}"))
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<_>>()
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(names, ["graph.html"]);
    }

    #[test]
    fn a_failed_rename_removes_the_temporary_file() {
        let tree = tree();
        let target = tree
            .create_dir("graph.html")
            .unwrap_or_else(|error| panic!("{error}"));
        tree.write_file("graph.html/inside", "x")
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(replace_file(&target, b"new").is_err());
        let names: Vec<_> = fs::read_dir(tree.path())
            .unwrap_or_else(|error| panic!("{error}"))
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .collect();
        assert_eq!(names, ["graph.html"]);
    }
}
