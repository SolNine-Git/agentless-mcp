use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use io::testing::Tree;
use serde::de::DeserializeOwned;

use crate::{EnvConfig, Services};

// The cases of one section of crates/app/fixtures/<file>.json.
pub(crate) fn cases<T: DeserializeOwned>(file: &str, section: &str) -> T {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
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

    // The start-up environment: PATH to find git, HOME and the cache home inside the sandbox.
    pub(crate) fn env(&self, cwd: &Path) -> EnvConfig {
        let path = std::env::var_os("PATH").expect("the tests find git through PATH");
        let home = self.path().join("home");
        let vars = [
            ("PATH", path),
            ("HOME", home.clone().into_os_string()),
            ("XDG_CACHE_HOME", home.join(".cache").into_os_string()),
            ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
            ("GIT_CONFIG_GLOBAL", OsString::from("/dev/null")),
        ];
        EnvConfig::new(
            vars.map(|(name, value)| (OsString::from(name), value)),
            cwd.to_path_buf(),
        )
    }

    pub(crate) fn services(&self, cwd: &Path) -> Services {
        Services::new(&self.env(cwd))
    }

    // Runs one setup git command in `dir` with a pinned identity and clock, and returns stdout.
    pub(crate) fn git(&self, dir: &Path, args: &[&str]) -> Vec<u8> {
        let pinned = [
            ("GIT_AUTHOR_NAME", "app tests"),
            ("GIT_AUTHOR_EMAIL", "app@example.invalid"),
            ("GIT_AUTHOR_DATE", "2026-02-01T00:00:00Z"),
            ("GIT_COMMITTER_NAME", "app tests"),
            ("GIT_COMMITTER_EMAIL", "app@example.invalid"),
            ("GIT_COMMITTER_DATE", "2026-02-01T00:00:00Z"),
        ];
        let mut env = self.env(dir).vars().to_vec();
        env.extend(pinned.map(|(name, value)| (OsString::from(name), OsString::from(value))));
        let mut command = vec![OsString::from("-C"), dir.as_os_str().to_owned()];
        command.extend(args.iter().map(OsString::from));
        tool("git", command, env)
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
            "symlink" => {
                let target = words[2].replace("<SANDBOX>", &self.path().display().to_string());
                let linked = self.tree.symlink(words[1], target);
                linked.unwrap_or_else(|error| panic!("{}: {error}", words[1]));
            }
            "remove" => drop(io::remove_file(&place).expect("a file to remove")),
            "mkdir" => drop(self.mkdir(words[1])),
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
