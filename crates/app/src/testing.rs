use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use io::testing::{Identity, Tree};

use crate::{EnvConfig, Services};

// The crate whose fixtures `cases` reads.
pub(crate) const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

// The commit identity of every setup repository. The app generator pins the same one.
const IDENTITY: Identity = Identity {
    name: "app tests",
    email: "app@example.invalid",
    date: "2026-02-01T00:00:00Z",
};

// A test's own directory, with an environment in which no git or cache config reaches the code.
pub(crate) struct Sandbox {
    tree: Tree,
}

impl Sandbox {
    pub(crate) fn new() -> Self {
        let tree = Tree::new().unwrap_or_else(|error| panic!("a temporary tree: {error}"));
        Self { tree }
    }

    pub(crate) fn path(&self) -> &Path {
        self.tree.path()
    }

    // Writes `bytes` to `relative`, with its missing parent directories.
    pub(crate) fn put(&self, relative: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        if let Some((parent, _)) = relative.rsplit_once('/') {
            self.tree
                .create_dir(parent)
                .unwrap_or_else(|error| panic!("{parent}: {error}"));
        }
        let written = self.tree.write_file(relative, bytes);
        written.unwrap_or_else(|error| panic!("{relative}: {error}"))
    }

    pub(crate) fn copy(&self, from: &Path, relative: &str) -> PathBuf {
        if let Some((parent, _)) = relative.rsplit_once('/') {
            self.mkdir(parent);
        }
        let copied = self.tree.copy_file(from, relative);
        copied.unwrap_or_else(|error| panic!("{relative}: {error}"))
    }

    pub(crate) fn mkdir(&self, relative: &str) -> PathBuf {
        self.tree.mkdir(relative)
    }

    pub(crate) fn chmod(&self, relative: &str, mode: u32) {
        self.tree.chmod(relative, mode);
    }

    // False for root, which writes into a directory whose mode forbids it.
    pub(crate) fn modes_bind(&self) -> bool {
        self.tree.modes_bind()
    }

    // The start-up environment: PATH to find git, HOME and the cache home inside the sandbox.
    pub(crate) fn env(&self, cwd: &Path) -> EnvConfig {
        EnvConfig::new(self.vars(), cwd.to_path_buf())
    }

    fn vars(&self) -> Vec<(OsString, OsString)> {
        io::testing::hermetic_env(&self.path().join("home"))
    }

    pub(crate) fn services(&self, cwd: &Path) -> Services {
        Services::new(&self.env(cwd))
    }

    // Runs one setup git command in `dir` with a pinned identity and clock, and returns stdout.
    pub(crate) fn git(&self, dir: &Path, args: &[&str]) -> Vec<u8> {
        io::testing::run_git(dir, args, &self.vars(), &IDENTITY)
    }

    // A new repository at `relative` with one commit of `files`.
    pub(crate) fn repo(&self, relative: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = self.mkdir(relative);
        self.apply_words(&["init", relative]);
        for (path, text) in files {
            self.put(&format!("{relative}/{path}"), text);
        }
        self.apply_words(&["commit", relative]);
        root
    }

    // Runs one scenario step of crates/app/fixtures/lint.json, as the generator ran it.
    pub(crate) fn apply(&self, step: &[serde_json::Value]) {
        let words: Vec<&str> = step
            .iter()
            .map(|word| word.as_str().unwrap_or_default())
            .collect();
        if words[0] == "repeat" {
            let count = step[3].as_u64().expect("a repeat count");
            let count = usize::try_from(count).expect("a count that fits");
            self.put(words[1], words[2].repeat(count));
        } else {
            self.apply_words(&words);
        }
    }

    fn apply_words(&self, words: &[&str]) {
        let place = self.path().join(words[1]);
        match words[0] {
            "write" => drop(self.put(words[1], words[2])),
            "bytes" => drop(self.put(words[1], latin(words[2]))),
            "latin_name" => {
                let name = OsString::from_vec(latin(words[1]));
                let written = self.tree.write_file(&name, words[2]);
                written.unwrap_or_else(|error| panic!("{name:?}: {error}"));
            }
            "latin_symlink" => {
                let target = OsString::from_vec(latin(words[2]));
                let linked = self.tree.symlink(words[1], target);
                linked.unwrap_or_else(|error| panic!("{}: {error}", words[1]));
            }
            "symlink" => {
                let target = words[2].replace("<SANDBOX>", &self.path().display().to_string());
                let linked = self.tree.symlink(words[1], target);
                linked.unwrap_or_else(|error| panic!("{}: {error}", words[1]));
            }
            "remove" => drop(io::remove_file(&place).expect("a file to remove")),
            "mkdir" => drop(self.mkdir(words[1])),
            "mtime" => set_mtime(&place, words[2]),
            _ => self.git_step(&place, words),
        }
    }

    fn git_step(&self, place: &Path, words: &[&str]) {
        match words[0] {
            "init" => drop(self.git(place, &["init", "--quiet", "-b", "main"])),
            "commit" => {
                self.git(place, &["add", "-A"]);
                self.git(
                    place,
                    &["commit", "--quiet", "--allow-empty", "-m", "fixture"],
                );
            }
            "gitlink" => {
                let head = self.git(place, &["rev-parse", "HEAD"]);
                let head = String::from_utf8(head).expect("a hex id");
                let info = format!("160000,{},{}", head.trim(), words[2]);
                self.git(place, &["update-index", "--add", "--cacheinfo", &info]);
            }
            "git" => drop(self.git(place, &words[2..])),
            other => panic!("no scenario step {other}"),
        }
    }
}

pub(crate) use io::testing::latin;

// Sets the modification time of `path` to `when`, an ISO 8601 time that touch -d reads.
pub(crate) fn set_mtime(path: &Path, when: &str) {
    let path_variable = std::env::var_os("PATH").expect("the tests find touch through PATH");
    let args = vec![
        OsString::from("-d"),
        OsString::from(when),
        path.as_os_str().to_owned(),
    ];
    tool("touch", args, vec![(OsString::from("PATH"), path_variable)]);
}

// Runs one setup program with a deadline and returns its stdout. A failed run ends the test.
pub(crate) fn tool(program: &str, args: Vec<OsString>, env: Vec<(OsString, OsString)>) -> Vec<u8> {
    let spec = io::Spec {
        program: OsString::from(program),
        args,
        cwd: PathBuf::from("."),
        env,
        timeout: Duration::from_secs(30),
        stdout_cap: 1 << 24,
        stderr_cap: 1 << 16,
    };
    let outcome = io::run(&spec);
    let io::Outcome::Exited {
        status,
        stdout,
        stderr,
    } = outcome
    else {
        panic!("{program} did not finish: {outcome:?}");
    };
    let shown = String::from_utf8_lossy(&stderr);
    assert!(status.success(), "{program} {:?}: {shown}", spec.args);
    stdout
}
