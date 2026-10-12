//! A temporary directory tree for tests, removed when it drops, a hermetic test environment, a
//! setup git runner with a pinned identity, and the fixture readers.

use std::ffi::OsString;
use std::fs::{self, Permissions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub use text::testing::{cases, document};

use crate::{Outcome, Spec, run};

static TREES: AtomicUsize = AtomicUsize::new(0);
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

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

    /// Returns whether a file mode can refuse this process. Root, or a process with
    /// `CAP_DAC_OVERRIDE`, reads a file whose mode is 000.
    ///
    /// # Panics
    ///
    /// Panics when the probe file cannot be written, changed or removed.
    #[must_use]
    pub fn modes_bind(&self) -> bool {
        let probe = self.write_file(".mode-probe", "");
        let probe = probe.unwrap_or_else(|error| panic!("a probe file: {error}"));
        self.chmod(".mode-probe", 0o000);
        let binds = fs::File::open(&probe).is_err();
        fs::remove_file(&probe).unwrap_or_else(|error| panic!("the probe file stays: {error}"));
        binds
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

/// The author and committer that a setup commit records, and its clock. The object ids of a
/// fixture depend on these values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The author and committer name.
    pub name: &'static str,
    /// The author and committer email.
    pub email: &'static str,
    /// The author and committer date, for example `2026-02-01T00:00:00Z`.
    pub date: &'static str,
}

impl Identity {
    /// Returns the `GIT_AUTHOR_*` and `GIT_COMMITTER_*` variables that pin this identity.
    #[must_use]
    pub fn vars(&self) -> [(OsString, OsString); 6] {
        [
            ("GIT_AUTHOR_NAME", self.name),
            ("GIT_AUTHOR_EMAIL", self.email),
            ("GIT_AUTHOR_DATE", self.date),
            ("GIT_COMMITTER_NAME", self.name),
            ("GIT_COMMITTER_EMAIL", self.email),
            ("GIT_COMMITTER_DATE", self.date),
        ]
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
    }
}

/// Returns an environment that no ambient config reaches: the PATH of the process, `home` as HOME,
/// `home/.cache` as `XDG_CACHE_HOME`, and no system or global git config.
///
/// # Panics
///
/// Panics when the process has no PATH.
#[must_use]
pub fn hermetic_env(home: &Path) -> Vec<(OsString, OsString)> {
    let path = std::env::var_os("PATH").expect("the tests find git through PATH");
    vec![
        (OsString::from("PATH"), path),
        (OsString::from("HOME"), home.as_os_str().to_owned()),
        (
            OsString::from("XDG_CACHE_HOME"),
            home.join(".cache").into_os_string(),
        ),
        (OsString::from("GIT_CONFIG_NOSYSTEM"), OsString::from("1")),
        (
            OsString::from("GIT_CONFIG_GLOBAL"),
            OsString::from("/dev/null"),
        ),
    ]
}

/// Runs one setup command `git -C <dir> <args>` with `env` and the pinned `identity`, under a
/// deadline, and returns its stdout.
///
/// # Panics
///
/// Panics with the outcome when git does not exit 0 before the deadline.
#[must_use]
pub fn run_git(
    dir: &Path,
    args: &[&str],
    env: &[(OsString, OsString)],
    identity: &Identity,
) -> Vec<u8> {
    let target = [OsString::from("-C"), dir.as_os_str().to_owned()];
    let spec = Spec {
        program: OsString::from("git"),
        args: target
            .into_iter()
            .chain(args.iter().map(OsString::from))
            .collect(),
        cwd: PathBuf::from("."),
        env: env.iter().cloned().chain(identity.vars()).collect(),
        timeout: GIT_TIMEOUT,
        stdout_cap: 1 << 24,
        stderr_cap: 1 << 16,
    };
    match run(&spec) {
        Outcome::Exited { status, stdout, .. } if status.success() => stdout,
        Outcome::Exited { status, stderr, .. } => {
            let shown = String::from_utf8_lossy(&stderr);
            panic!("git {args:?} in {}: {status}: {shown}", dir.display())
        }
        outcome => panic!("git {args:?} in {}: {outcome:?}", dir.display()),
    }
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

    const IDENTITY: Identity = Identity {
        name: "io tests",
        email: "io@example.invalid",
        date: "2026-02-01T00:00:00Z",
    };

    #[test]
    fn a_setup_commit_records_the_pinned_identity_and_clock() {
        let tree = Tree::new().expect("the temporary directory takes a tree");
        let repo = tree.mkdir("repo");
        let env = hermetic_env(&tree.join("home"));
        let _ = run_git(&repo, &["init", "--quiet", "-b", "main"], &env, &IDENTITY);
        let commit = ["commit", "--quiet", "--allow-empty", "-m", "fixture"];
        let _ = run_git(&repo, &commit, &env, &IDENTITY);
        // Epoch seconds, because the ISO form of a UTC date differs between git versions.
        let format = "--format=%an|%ae|%at|%cn|%ce|%ct";
        let shown = run_git(&repo, &["log", "-1", format], &env, &IDENTITY);
        let expected = "io tests|io@example.invalid|1769904000|\
                        io tests|io@example.invalid|1769904000\n";
        assert_eq!(String::from_utf8_lossy(&shown), expected);
    }

    #[test]
    #[should_panic(expected = "git [\"no-such-command\"]")]
    fn a_failed_setup_command_names_its_arguments() {
        let tree = Tree::new().expect("the temporary directory takes a tree");
        let env = hermetic_env(&tree.join("home"));
        let _ = run_git(tree.path(), &["no-such-command"], &env, &IDENTITY);
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
