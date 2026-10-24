//! The owner and mode test of a directory that other users can reach, such as one under /tmp.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rustix::process::geteuid;

/// Returns true when `path` is a directory that the effective user owns and nobody else can write.
///
/// # Errors
///
/// Returns the error of the stat, which does not follow a symlink at `path`.
pub fn is_private_directory(path: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    let is_dir = metadata.file_type().is_dir();
    let owned = owned_directory(is_dir, metadata.uid(), geteuid().as_raw());
    Ok(owned && !others_can_write(metadata.mode()))
}

// The file type of a symlink's own metadata is never a directory.
fn owned_directory(is_dir: bool, owner: u32, user: u32) -> bool {
    is_dir && owner == user
}

// A user who can write the directory can plant or remove entries in it, whoever owns it.
fn others_can_write(mode: u32) -> bool {
    mode & 0o022 != 0
}

#[cfg(test)]
mod tests {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt;

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
    fn the_group_and_other_write_bits_refuse_a_directory() {
        for mode in [0o700, 0o755, 0o555, 0o1755] {
            assert!(!others_can_write(mode), "{mode:o}");
        }
        for mode in [0o720, 0o702, 0o775, 0o777, 0o1777] {
            assert!(others_can_write(mode), "{mode:o}");
        }
    }

    #[test]
    fn only_a_directory_that_others_cannot_write_passes() {
        let tree = tree();
        let modes = [(0o700, true), (0o755, true), (0o775, false), (0o770, false)];
        let world = [(0o777, false), (0o703, false)];
        for (mode, passes) in modes.into_iter().chain(world) {
            let directory = tree
                .create_dir(format!("d{mode:o}"))
                .unwrap_or_else(|error| panic!("{error}"));
            fs::set_permissions(&directory, Permissions::from_mode(mode))
                .unwrap_or_else(|error| panic!("{error}"));
            let found = is_private_directory(&directory).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(found, passes, "mode {mode:o}");
        }
    }

    #[test]
    fn a_symlink_to_the_users_directory_is_refused() {
        let tree = tree();
        let real = tree
            .create_dir("real")
            .unwrap_or_else(|error| panic!("{error}"));
        fs::set_permissions(&real, Permissions::from_mode(0o755))
            .unwrap_or_else(|error| panic!("{error}"));
        let link = tree
            .symlink("link", &real)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(is_private_directory(&real).unwrap_or_else(|error| panic!("{error}")));
        assert!(!is_private_directory(&link).unwrap_or_else(|error| panic!("{error}")));
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let tree = tree();
        assert!(is_private_directory(&tree.join("missing")).is_err());
    }
}
