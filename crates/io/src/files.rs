//! Single-file changes: remove a file, and mark a file as used now.

use std::fs::{self, File, FileTimes};
use std::io;
use std::path::Path;
use std::time::SystemTime;

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
    fn touch_returns_the_error_for_a_missing_file() {
        let tree = tree();
        let refused = touch(&tree.join("missing"));
        assert_eq!(
            refused.map_err(|error| error.kind()),
            Err(io::ErrorKind::NotFound)
        );
    }
}
