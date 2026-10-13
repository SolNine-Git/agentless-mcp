//! Single-file changes: remove a file, mark a file as used now, and create an owner-only
//! directory.

use std::fs::{self, DirBuilder, File, FileTimes, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const PRIVATE_MODE: u32 = 0o700;

/// Creates `directory` with mode 0700 and its missing parents with the default mode, then sets
/// mode 0700 on its parent and on itself. It returns each mode change that failed.
///
/// # Errors
///
/// Returns the error of a creation that failed, or of a path that exists and is not a directory.
pub fn create_private_dir(directory: &Path) -> io::Result<Vec<(PathBuf, io::Error)>> {
    if let Some(parent) = directory.parent() {
        fs::create_dir_all(parent)?;
    }
    match DirBuilder::new().mode(PRIVATE_MODE).create(directory) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists || !directory.is_dir() => {
            return Err(error);
        }
        _ => {}
    }
    let levels = directory.parent().into_iter().chain([directory]);
    Ok(levels
        .filter_map(|path| {
            let changed = fs::set_permissions(path, Permissions::from_mode(PRIVATE_MODE));
            changed.err().map(|error| (path.to_path_buf(), error))
        })
        .collect())
}

/// Removes the file `path`, and returns false when nothing was there.
///
/// # Errors
///
/// Returns each remove error except a missing file.
pub fn remove_file(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Sets the access and modification times of the file that `path` names to now, following
/// symlinks. The file must be readable, and the caller must own it.
///
/// # Errors
///
/// Returns the open error or the change error.
pub fn touch(path: &Path) -> io::Result<()> {
    let now = SystemTime::now();
    let times = FileTimes::new().set_accessed(now).set_modified(now);
    File::open(path)?.set_times(times)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::tree;
    use std::time::Duration;

    #[test]
    fn remove_file_says_whether_a_file_was_there() {
        let tree = tree();
        let path = tree.write_file("f", "x").expect("the file is written");
        assert!(remove_file(&path).expect("the remove succeeds"));
        assert!(!path.exists());
        assert!(!remove_file(&path).expect("a missing file is no error"));
    }

    #[test]
    fn remove_file_returns_other_errors() {
        let tree = tree();
        let directory = tree.create_dir("d").expect("the directory is created");
        assert!(
            remove_file(&directory).is_err(),
            "a directory is not a file"
        );
    }

    #[test]
    fn touch_moves_both_times_to_now_through_a_symlink() {
        let tree = tree();
        let path = tree.write_file("f", "x").expect("the file is written");
        let link = tree.symlink("l", "f").expect("the symlink is created");
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let times = FileTimes::new().set_accessed(old).set_modified(old);
        File::open(&path)
            .and_then(|file| file.set_times(times))
            .expect("the old times are set");
        let before = SystemTime::now();
        touch(&link).expect("the touch succeeds");
        let metadata = fs::metadata(&path).expect("the file exists");
        let modified = metadata
            .modified()
            .expect("the platform has modification times");
        let accessed = metadata.accessed().expect("the platform has access times");
        let recent = before - Duration::from_secs(2);
        assert!(
            modified >= recent && accessed >= recent,
            "{modified:?} {accessed:?}"
        );
    }

    #[test]
    fn a_private_directory_is_owner_only_at_both_levels() {
        let tree = tree();
        let parent = tree.create_dir("cache").expect("the parent is created");
        fs::set_permissions(&parent, Permissions::from_mode(0o755)).expect("the mode is set");
        let directory = parent.join("key");

        let failed = create_private_dir(&directory).expect("the directory is created");
        let again = create_private_dir(&directory).expect("an existing directory is kept");

        assert!(failed.is_empty() && again.is_empty());
        for path in [&parent, &directory] {
            let mode = fs::metadata(path)
                .expect("the path exists")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "{}", path.display());
        }
    }

    #[test]
    fn missing_parents_take_the_default_mode() {
        let tree = tree();
        let plain = tree
            .create_dir("plain")
            .expect("a default directory is created");
        let top = tree.join("top");
        let directory = top.join("cache/key");

        create_private_dir(&directory).expect("the directories are created");

        let mode = |path: &Path| {
            fs::metadata(path)
                .expect("the path exists")
                .permissions()
                .mode()
        };
        assert_eq!(
            mode(&top),
            mode(&plain),
            "only the leaf and its parent are restricted"
        );
    }

    #[test]
    fn a_file_in_the_place_of_the_directory_is_an_error() {
        let tree = tree();
        let path = tree.write_file("key", "x").expect("the file is written");

        let refused = create_private_dir(&path).expect_err("a file is not a directory");

        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn touch_returns_the_error_for_a_missing_file() {
        let tree = tree();
        let refused = touch(&tree.join("missing"));
        assert_eq!(
            refused.map_err(|error| error.kind()),
            Err(io::ErrorKind::NotFound)
        );
    }
}
