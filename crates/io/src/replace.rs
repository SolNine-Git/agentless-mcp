//! Atomic replacement of one file: an owner-only parent, a private temporary file beside the
//! target, a flush to disk, then a rename over the target. The new file is private, or keeps the
//! mode of the file it replaces.

use std::fs::{self, File, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::files::create_missing_owner_dir;
use crate::random::{create_named, sibling_prefix};

const FILE_MODE: u32 = 0o600;
const PERMISSION_BITS: u32 = 0o777;

/// Replaces the file `target` with `bytes` in one rename: the parent is created with mode 0700
/// when missing, the new file has mode 0600, and a rename never follows a symlink at `target`.
/// The temporary file is removed when a step fails.
///
/// # Errors
///
/// Returns the error of the step that failed: the parent, the temporary file, the mode, the
/// write, the flush, or the rename.
pub fn replace_file(target: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with_mode(target, bytes, FILE_MODE)
}

/// Replaces the file `target` with `bytes` as [`replace_file`] does, but the new file has the
/// permission bits of the file it replaces. A new file has mode 0600.
///
/// # Errors
///
/// Returns the error of the read of the old file's mode, or of a step of [`replace_file`].
pub fn replace_file_keeping_mode(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mode = match fs::metadata(target) {
        Ok(metadata) => metadata.permissions().mode() & PERMISSION_BITS,
        Err(error) if error.kind() == io::ErrorKind::NotFound => FILE_MODE,
        Err(error) => return Err(error),
    };
    replace_with_mode(target, bytes, mode)
}

fn replace_with_mode(target: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
        let message = format!("{} names no file in a directory", target.display());
        return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
    };
    create_missing_owner_dir(parent)?;
    let (temporary, file) = create_named(parent, &sibling_prefix(name), "")?;
    let replaced = write_with_mode(file, bytes, mode).and_then(|()| fs::rename(&temporary, target));
    // The failed step's error is the one returned; a failed cleanup leaves a hidden file.
    if replaced.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    replaced
}

// The mode goes on the open file: a path that another user swapped in is never followed.
fn write_with_mode(mut file: File, bytes: &[u8], mode: u32) -> io::Result<()> {
    file.set_permissions(Permissions::from_mode(mode))?;
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
    fn a_kept_mode_is_the_replaced_files_and_a_new_file_gets_0600() {
        let tree = tree();
        let target = tree
            .write_file("settings.json", "old")
            .unwrap_or_else(|error| panic!("{error}"));
        tree.set_mode("settings.json", 0o640)
            .unwrap_or_else(|error| panic!("{error}"));
        replace_file_keeping_mode(&target, b"new").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(fs::read(&target).ok(), Some(b"new".to_vec()));
        assert_eq!(mode(&target), 0o640);
        let fresh = tree.join("fresh/settings.json");
        replace_file_keeping_mode(&fresh, b"{}").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(mode(&fresh), FILE_MODE);
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
