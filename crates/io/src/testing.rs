//! A temporary directory tree for tests, removed when it drops, and the fixture section reader.

use std::fs::{self, Permissions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::de::DeserializeOwned;

static TREES: AtomicUsize = AtomicUsize::new(0);

/// A new directory under the temporary directory. The drop removes it and everything in it.
#[derive(Debug)]
pub struct Tree {
    root: PathBuf,
}

impl Tree {
    /// Creates an empty directory under the temporary directory, with a name no other tree has.
    ///
    /// # Errors
    ///
    /// Returns the error of the create or of the resolution of its path.
    pub fn new() -> io::Result<Self> {
        let count = TREES.fetch_add(1, Ordering::Relaxed);
        let name = format!("io-test-{}-{count}", std::process::id());
        let created = std::env::temp_dir().join(name);
        fs::create_dir(&created)?;
        let root = fs::canonicalize(&created)?;
        Ok(Self { root })
    }

    /// Returns the resolved path of the directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Returns the path of `relative` below the directory.
    #[must_use]
    pub fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }

    /// Creates the directory `relative` and its missing parents, and returns its path.
    ///
    /// # Errors
    ///
    /// Returns an error for an absolute `relative`, or the create error.
    pub fn create_dir(&self, relative: impl AsRef<Path>) -> io::Result<PathBuf> {
        let path = self.below(relative.as_ref())?;
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    /// Writes `contents` to the file `relative`, which it creates or truncates, and returns its
    /// path.
    ///
    /// # Errors
    ///
    /// Returns an error for an absolute `relative`, or the write error.
    pub fn write_file(
        &self,
        relative: impl AsRef<Path>,
        contents: impl AsRef<[u8]>,
    ) -> io::Result<PathBuf> {
        let path = self.below(relative.as_ref())?;
        fs::write(&path, contents)?;
        Ok(path)
    }

    /// Copies the file `from` to the file `relative`, and returns the path of the copy.
    ///
    /// # Errors
    ///
    /// Returns an error for an absolute `relative`, or the copy error.
    pub fn copy_file(
        &self,
        from: impl AsRef<Path>,
        relative: impl AsRef<Path>,
    ) -> io::Result<PathBuf> {
        let path = self.below(relative.as_ref())?;
        fs::copy(from, &path)?;
        Ok(path)
    }

    /// Creates the symlink `relative` with the target text `target`, and returns its path.
    ///
    /// # Errors
    ///
    /// Returns an error for an absolute `relative`, or the create error.
    pub fn symlink(
        &self,
        relative: impl AsRef<Path>,
        target: impl AsRef<Path>,
    ) -> io::Result<PathBuf> {
        let path = self.below(relative.as_ref())?;
        std::os::unix::fs::symlink(target, &path)?;
        Ok(path)
    }

    /// Sets the permission bits of `relative` to `mode`, following a symlink.
    ///
    /// # Errors
    ///
    /// Returns an error for an absolute `relative`, or the change error.
    pub fn set_mode(&self, relative: impl AsRef<Path>, mode: u32) -> io::Result<()> {
        let path = self.below(relative.as_ref())?;
        fs::set_permissions(path, Permissions::from_mode(mode))
    }

    /// Creates the directory `relative` and its missing parents, and returns its path.
    ///
    /// # Panics
    ///
    /// Panics with the path when the create fails.
    #[must_use]
    pub fn mkdir(&self, relative: &str) -> PathBuf {
        let made = self.create_dir(relative);
        made.unwrap_or_else(|error| panic!("{relative}: {error}"))
    }

    /// Sets the permission bits of `relative` to `mode`, following a symlink.
    ///
    /// # Panics
    ///
    /// Panics with the path when the change fails.
    pub fn chmod(&self, relative: &str, mode: u32) {
        let set = self.set_mode(relative, mode);
        set.unwrap_or_else(|error| panic!("{relative}: {error}"));
    }

    // An absolute path would replace the root in a join and land outside the tree.
    fn below(&self, relative: &Path) -> io::Result<PathBuf> {
        if relative.is_absolute() {
            let message = format!("{} is not a path below the tree", relative.display());
            return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        }
        Ok(self.root.join(relative))
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("the test tree {} stays: {error}", self.root.display());
        }
    }
}

/// Returns the section `section` of the fixture file `<manifest_dir>/fixtures/<file>.json`.
///
/// # Panics
///
/// Panics when the file cannot be read or parsed, has no such section, or the section is no `T`.
#[must_use]
pub fn cases<T: DeserializeOwned>(manifest_dir: &str, file: &str, section: &str) -> T {
    let path = Path::new(manifest_dir)
        .join("fixtures")
        .join(format!("{file}.json"));
    let shown = path.display();
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let mut sections: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let value = sections
        .remove(section)
        .unwrap_or_else(|| panic!("{shown}: no section {section}"));
    serde_json::from_value(value).unwrap_or_else(|error| panic!("{shown}: {section}: {error}"))
}

/// Returns the bytes that `text` stores, one byte per character, as a fixture stores bytes.
///
/// # Panics
///
/// Panics when `text` holds a character above U+00FF.
#[must_use]
pub fn latin(text: &str) -> Vec<u8> {
    let bytes: Result<Vec<u8>, _> = text.chars().map(u8::try_from).collect();
    bytes.unwrap_or_else(|error| panic!("{text:?} holds a character above U+00FF: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_builds_files_and_leaves_nothing_after_the_drop() {
        let tree = Tree::new().expect("the temporary directory takes a tree");
        let root = tree.path().to_path_buf();
        tree.create_dir("a/b").expect("the directories are created");
        let file = tree
            .write_file("a/b/f", b"bytes")
            .expect("the file is written");
        let copy = tree.copy_file(&file, "a/copy").expect("the file is copied");
        tree.symlink("link", "a/copy")
            .expect("the symlink is created");
        tree.set_mode("a/copy", 0o600).expect("the mode is set");
        assert_eq!(
            fs::read(tree.join("link")).expect("the link is read"),
            b"bytes"
        );
        let mode = fs::metadata(&copy)
            .expect("the copy exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        drop(tree);
        assert!(!root.exists(), "the drop removes the tree");
    }

    #[test]
    fn an_absolute_path_is_refused() {
        let tree = Tree::new().expect("the temporary directory takes a tree");
        let refused = tree.write_file("/tmp/io-tree-escape", "x").map(drop);
        assert_eq!(
            refused.map_err(|error| error.kind()),
            Err(io::ErrorKind::InvalidInput)
        );
    }
}
