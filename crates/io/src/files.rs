//! Single-file changes: write a file, remove a file or a tree, mark a file as used now, and create
//! an owner-only directory.

use std::fs::{self, DirBuilder, File, FileTimes, OpenOptions, Permissions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::native::close_checked;

const PRIVATE_MODE: u32 = 0o700;
// The mode a new file asks for; the umask removes bits from it.
const PLAIN_FILE_MODE: u32 = 0o666;

/// Creates `directory` with mode 0700 and its missing parents with the default mode, then sets
/// mode 0700 on its parent and on itself. It returns each mode change that failed, where
/// [`create_owner_dir`] fails on it.
///
/// # Errors
///
/// Returns the error of a creation that failed, or of a path that exists and is not a directory.
pub fn create_private_dir(directory: &Path) -> io::Result<Vec<(PathBuf, io::Error)>> {
    make_dir(directory)?;
    let levels = directory.parent().into_iter().chain([directory]);
    Ok(levels
        .filter_map(|path| {
            let changed = fs::set_permissions(path, Permissions::from_mode(PRIVATE_MODE));
            changed.err().map(|error| (path.to_path_buf(), error))
        })
        .collect())
}

/// Creates `directory` with mode 0700 and its missing parents with the default mode, then sets
/// mode 0700 on `directory` alone. Unlike [`create_private_dir`], it leaves the parent's mode.
///
/// # Errors
///
/// Returns the error of a creation or of the mode change, or of a path that exists and is not a
/// directory.
pub fn create_owner_dir(directory: &Path) -> io::Result<()> {
    make_dir(directory)?;
    fs::set_permissions(directory, Permissions::from_mode(PRIVATE_MODE))
}

// The missing parents take the default mode. A new `directory` gets 0700 at its creation, so it
// is never briefly at the umask's mode, and an existing one keeps its mode.
pub(crate) fn make_dir(directory: &Path) -> io::Result<()> {
    if let Some(parent) = directory.parent() {
        fs::create_dir_all(parent)?;
    }
    match DirBuilder::new().mode(PRIVATE_MODE).create(directory) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists || !directory.is_dir() => {
            Err(error)
        }
        _ => Ok(()),
    }
}

/// Writes `bytes` to the file `path`: a new file gets mode 0666 less the umask, and an existing
/// file is truncated first. There is no temporary file. A regular file is flushed to disk, and the
/// close is checked, so a deferred write error is returned.
///
/// # Errors
///
/// Returns the error of the open, of a write, of the flush, or of the close.
pub fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(PLAIN_FILE_MODE)
        .open(path)?;
    file.write_all(bytes)?;
    sync_written(&file)?;
    close_checked(file)
}

// A deferred write error (NFS, a disk quota) shows only in the flush or in the close, so a writer
// flushes, then checks the close. A device or a pipe has nothing to flush.
pub(crate) fn sync_written(file: &File) -> io::Result<()> {
    if !file.metadata()?.is_file() {
        return Ok(());
    }
    flushed(file.sync_data())
}

// A file system that cannot flush answers Unsupported or InvalidInput (EINVAL): its data stays as
// the writes left it.
pub(crate) fn flushed(synced: io::Result<()>) -> io::Result<()> {
    match synced {
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::Unsupported | ErrorKind::InvalidInput
            ) =>
        {
            Ok(())
        }
        other => other,
    }
}

/// Removes the directory `path` and everything in it.
///
/// # Errors
///
/// Returns the first error of the removal.
pub fn remove_tree(path: &Path) -> io::Result<()> {
    fs::remove_dir_all(path)
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

    fn mode_of(path: &Path) -> u32 {
        let metadata = fs::metadata(path).expect("the path exists");
        metadata.permissions().mode() & 0o7777
    }

    #[test]
    fn an_owner_dir_restricts_the_leaf_alone() {
        let tree = tree();
        let parent = tree.create_dir("cache").expect("the parent is created");
        fs::set_permissions(&parent, Permissions::from_mode(0o755)).expect("the mode is set");
        let directory = parent.join("worktrees");
        create_owner_dir(&directory).expect("the directory is created");
        fs::set_permissions(&directory, Permissions::from_mode(0o750)).expect("the mode is set");
        create_owner_dir(&directory).expect("an existing directory is kept");
        assert_eq!((mode_of(&parent), mode_of(&directory)), (0o755, 0o700));
        let file = tree.write_file("plain", "x").expect("the file is written");
        let refused = create_owner_dir(&file).expect_err("a file is not a directory");
        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn a_plain_write_takes_the_umask_mode_and_truncates() {
        let tree = tree();
        let reference = tree
            .write_file("reference", "")
            .expect("the file is written");
        let path = tree.join("verdicts.jsonl");
        write_file(&path, b"a much longer first document\n").expect("the write succeeds");
        write_file(&path, b"second\n").expect("the write succeeds");
        assert_eq!(fs::read(&path).expect("the file reads"), b"second\n");
        assert_eq!(mode_of(&path), mode_of(&reference));
        let names: Vec<_> = fs::read_dir(tree.path())
            .expect("the tree lists")
            .map(|entry| entry.expect("an entry").file_name())
            .collect();
        assert_eq!(names.len(), 2, "a temporary file stayed: {names:?}");
    }

    #[test]
    fn a_plain_write_keeps_the_mode_of_an_existing_file() {
        let tree = tree();
        let path = tree.write_file("out", "old").expect("the file is written");
        tree.chmod("out", 0o640);
        write_file(&path, b"new").expect("the write succeeds");
        assert_eq!(mode_of(&path), 0o640);
    }

    #[test]
    fn a_plain_write_returns_the_os_error() {
        let tree = tree();
        let directory = tree.create_dir("d").expect("the directory is created");
        let refused = write_file(&directory, b"x").expect_err("a directory is not a file");
        assert_eq!(crate::strerror(&refused), "Is a directory");
        let missing = write_file(&tree.join("missing/out"), b"x").expect_err("no parent");
        assert_eq!(crate::strerror(&missing), "No such file or directory");
    }

    #[test]
    fn a_device_takes_a_plain_write_with_no_flush() {
        write_file(Path::new("/dev/null"), b"verdicts\n").expect("/dev/null takes the write");
    }

    #[test]
    fn only_a_file_system_with_no_flush_counts_as_flushed() {
        for code in [libc::EINVAL, libc::EOPNOTSUPP, libc::ENOSYS] {
            let refused = Err(io::Error::from_raw_os_error(code));
            assert!(flushed(refused).is_ok(), "errno {code}");
        }
        let quota = || io::Error::from_raw_os_error(libc::EDQUOT);
        let error = flushed(Err(quota())).expect_err("a deferred write error surfaces");
        // The C library names the errno: macOS says "Disc quota exceeded".
        assert_eq!(crate::strerror(&error), crate::strerror(&quota()));
    }

    #[test]
    fn a_tree_is_removed_with_everything_in_it() {
        let tree = tree();
        tree.create_dir("wt/a")
            .expect("the directories are created");
        tree.write_file("wt/a/b.txt", "x")
            .expect("the file is written");
        remove_tree(&tree.join("wt")).expect("the tree is removed");
        assert!(!tree.join("wt").exists());
        let missing = remove_tree(&tree.join("wt")).expect_err("nothing is there");
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);
    }
}
