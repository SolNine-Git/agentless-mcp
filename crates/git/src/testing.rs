use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use io::testing::{Identity, Tree};
use serde::de::DeserializeOwned;

use crate::Git;

// The commit identity and clock of every setup repository, so each object id is the same.
const IDENTITY: Identity = Identity {
    name: "agentless-mcp tests",
    email: "tests@example.invalid",
    date: "2026-01-01T00:00:00Z",
};

pub(crate) fn cases<T: DeserializeOwned>(file: &str, section: &str) -> T {
    io::testing::cases(env!("CARGO_MANIFEST_DIR"), file, section)
}

// A PATH value that finds the programs in `bin` before the ones the tests run with.
pub(crate) fn path_before(bin: PathBuf) -> OsString {
    let real = std::env::var_os("PATH").expect("the tests find programs through PATH");
    let entries = std::iter::once(bin).chain(std::env::split_paths(&real));
    std::env::join_paths(entries).expect("a PATH value")
}

pub(crate) use io::testing::latin;

// A temporary tree that the test owns, built and removed through the io crate.
pub(crate) struct Scratch {
    tree: Tree,
    pub(crate) root: PathBuf,
}

impl Scratch {
    pub(crate) fn new() -> Self {
        let tree = Tree::new().expect("the temporary directory takes a tree");
        let root = tree.path().to_path_buf();
        Self { tree, root }
    }

    pub(crate) fn write(&self, relative: &str, text: &str) -> PathBuf {
        self.write_bytes(OsStr::new(relative), text.as_bytes())
    }

    // Writes the file below the root, and creates its missing parents first.
    pub(crate) fn write_bytes(&self, relative: &OsStr, bytes: &[u8]) -> PathBuf {
        let relative = Path::new(relative);
        if let Some(parent) = relative
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            self.tree
                .create_dir(parent)
                .unwrap_or_else(|error| panic!("{parent:?}: {error}"));
        }
        let written = self.tree.write_file(relative, bytes);
        written.unwrap_or_else(|error| panic!("{relative:?}: {error}"))
    }

    pub(crate) fn create_dir(&self, relative: &str) -> PathBuf {
        self.tree.mkdir(relative)
    }

    pub(crate) fn symlink(&self, relative: &str, target: &Path) {
        let linked = self.tree.symlink(relative, target);
        linked.unwrap_or_else(|error| panic!("{relative}: {error}"));
    }

    pub(crate) fn set_mode(&self, relative: &str, mode: u32) {
        self.tree.chmod(relative, mode);
    }

    // False for root, which reads a directory whose mode is 000.
    pub(crate) fn modes_bind(&self) -> bool {
        self.tree.modes_bind()
    }

    // The base environment a test hands to `Git::new`: no config file reaches git.
    pub(crate) fn env(&self) -> Vec<(OsString, OsString)> {
        self.env_with(&[])
    }

    // `extra` adds names and replaces the fixed ones.
    pub(crate) fn env_with(&self, extra: &[(&str, &OsStr)]) -> Vec<(OsString, OsString)> {
        let path = std::env::var_os("PATH").expect("the tests find git through PATH");
        let fixed = [
            ("PATH", path.as_os_str()),
            ("HOME", self.root.as_os_str()),
            ("GIT_CONFIG_NOSYSTEM", OsStr::new("1")),
            ("GIT_CONFIG_GLOBAL", OsStr::new("/dev/null")),
        ];
        let replaced = |name: &&str| extra.iter().any(|(added, _)| added == name);
        let kept = fixed.into_iter().filter(|(name, _)| !replaced(name));
        kept.chain(extra.iter().copied())
            .map(|(name, value)| (OsString::from(name), value.to_owned()))
            .collect()
    }

    pub(crate) fn git(&self) -> Git {
        Git::new(self.env())
    }

    // A repository with a pinned identity and clock and one commit of `files`.
    pub(crate) fn repo(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = self.init(name);
        for (relative, text) in files {
            self.write(&format!("{name}/{relative}"), text);
        }
        self.commit_all(&root, "fixture");
        root
    }

    // A new repository with no commit; git creates the directory.
    pub(crate) fn init(&self, name: &str) -> PathBuf {
        let root = self.root.join(name);
        let target = root.to_str().expect("a UTF-8 scratch path");
        self.run_git(&self.root, &["init", "--quiet", "-b", "main", target]);
        root
    }

    pub(crate) fn commit_all(&self, root: &Path, message: &str) {
        self.run_git(root, &["add", "-A"]);
        self.run_git(root, &["commit", "--quiet", "-m", message]);
    }

    // Runs one setup git command in `dir`, with a deadline, and returns its stdout.
    pub(crate) fn run_git(&self, dir: &Path, args: &[&str]) -> Vec<u8> {
        io::testing::run_git(dir, args, &self.env(), &IDENTITY)
    }

    // A directory that holds one program named `git` with `body` and `mode`, for a PATH entry.
    pub(crate) fn fake_git(&self, body: &str, mode: u32) -> PathBuf {
        self.write("fake-bin/git", body);
        self.set_mode("fake-bin/git", mode);
        self.root.join("fake-bin")
    }
}
