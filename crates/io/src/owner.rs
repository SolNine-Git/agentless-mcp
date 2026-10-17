//! The owner test of a directory that other users can reach, such as one under /tmp.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rustix::process::geteuid;

/// Returns true when `path` is a directory that the effective user owns, and false for any other
/// entry, a symlink to a directory included.
///
/// # Errors
///
/// Returns the error of the stat, which does not follow a symlink at `path`.
pub fn is_own_directory(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    let is_dir = metadata.file_type().is_dir();
    Ok(owned_directory(is_dir, metadata.uid(), geteuid().as_raw()))
}

// The file type of a symlink's own metadata is never a directory.
fn owned_directory(is_dir: bool, owner: u32, user: u32) -> bool {
    is_dir && owner == user
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::tree;

    #[test]
    fn only_a_directory_of_the_user_passes_the_owner_rule() {
        assert!(owned_directory(true, 1000, 1000));
        assert!(!owned_directory(true, 0, 1000), "another user's directory");
        assert!(
            !owned_directory(false, 1000, 1000),
            "the user's file or symlink"
        );
    }

    #[test]
    fn a_symlink_to_the_users_directory_is_refused() {
        let tree = tree();
        let real = tree
            .create_dir("real")
            .unwrap_or_else(|error| panic!("{error}"));
        let link = tree
            .symlink("link", &real)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(is_own_directory(&real).unwrap_or_else(|error| panic!("{error}")));
        assert!(!is_own_directory(&link).unwrap_or_else(|error| panic!("{error}")));
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let tree = tree();
        assert!(is_own_directory(&tree.join("missing")).is_err());
    }
}
