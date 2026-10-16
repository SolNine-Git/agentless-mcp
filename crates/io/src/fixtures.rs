//! The fixture files, and test trees built from their layouts.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::testing::{self, Tree};
use crate::{Outcome, Spec, run};

const MKFIFO_TIMEOUT: Duration = Duration::from_secs(10);
// The marker that turns a run of this test binary into the child that one test starts.
const CHILD: &str = "IO_TEST_CHILD";
const CHILD_TIMEOUT: Duration = Duration::from_secs(60);

// True in the child process that `run_child(name)` starts.
pub(crate) fn is_child(name: &str) -> bool {
    std::env::var_os(CHILD).is_some_and(|value| value == name)
}

// Runs the test `name` of this binary alone in a child process, with the PATH and TMPDIR of this
// process, and returns how the child ended and its stderr.
pub(crate) fn run_child(name: &str) -> (std::process::ExitStatus, String) {
    let binary = std::env::current_exe().expect("the test binary has a path");
    let kept = ["PATH", "TMPDIR"]
        .into_iter()
        .filter_map(|key| std::env::var_os(key).map(|value| (OsString::from(key), value)));
    let spec = Spec {
        program: binary.into_os_string(),
        args: ["--exact", name, "--nocapture", "--test-threads", "1"]
            .map(OsString::from)
            .to_vec(),
        cwd: PathBuf::from("/"),
        env: kept
            .chain([(OsString::from(CHILD), OsString::from(name))])
            .collect(),
        timeout: CHILD_TIMEOUT,
        stdout_cap: 1 << 16,
        stderr_cap: 1 << 16,
    };
    match run(&spec) {
        Outcome::Exited { status, stderr, .. } => {
            (status, String::from_utf8_lossy(&stderr).into_owned())
        }
        outcome => panic!("the child test {name} did not finish: {outcome:?}"),
    }
}

pub(crate) fn fixture<T: DeserializeOwned>(file: &str, section: &str) -> T {
    testing::cases(env!("CARGO_MANIFEST_DIR"), file, section)
}

pub(crate) fn tree() -> Tree {
    Tree::new().unwrap_or_else(|error| panic!("the temporary directory takes no tree: {error}"))
}

// Each layout entry is a kind, a path below the tree, and the kind's arguments.
pub(crate) fn build(layout: &[Value]) -> Tree {
    let tree = tree();
    for entry in layout {
        make(&tree, entry).unwrap_or_else(|error| panic!("{entry}: {error}"));
    }
    tree
}

// The fixtures name the tree's path {base}.
pub(crate) trait Placed {
    fn placed(&self, text: &str) -> String;
    fn unplaced(&self, text: &str) -> PathBuf;
}

impl Placed for Tree {
    fn placed(&self, text: &str) -> String {
        text.replace(&*self.path().to_string_lossy(), "{base}")
    }

    fn unplaced(&self, text: &str) -> PathBuf {
        PathBuf::from(text.replace("{base}", &self.path().to_string_lossy()))
    }
}

fn make(tree: &Tree, entry: &Value) -> std::io::Result<()> {
    let text = |index: usize| entry[index].as_str().unwrap_or_else(|| panic!("{entry}"));
    let path = text(1);
    match text(0) {
        "dir" => tree.create_dir(path).map(drop),
        "file" => tree.write_file(path, text(2)).map(drop),
        "bytes" => tree.write_file(path, bytes(&entry[2])).map(drop),
        "padded" => tree.write_file(path, padded(text(2), &entry[3])).map(drop),
        "link" => tree.symlink(path, tree.unplaced(text(2))).map(drop),
        "fifo" => mkfifo(&tree.join(path)),
        "mode" => tree.set_mode(path, mode(&entry[2])),
        kind => panic!("unknown layout entry kind {kind}"),
    }
}

// A layout whose `mode` entry refuses a read expects that refusal only where modes bind.
pub(crate) fn has_mode(layout: &[Value]) -> bool {
    layout.iter().any(|entry| entry[0] == "mode")
}

fn mode(value: &Value) -> u32 {
    let mode = value.as_u64().and_then(|mode| u32::try_from(mode).ok());
    mode.unwrap_or_else(|| panic!("a mode: {value}"))
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::from_value(value.clone()).unwrap_or_else(|error| panic!("{value}: {error}"))
}

fn padded(text: &str, size: &Value) -> Vec<u8> {
    let size = size.as_u64().and_then(|size| usize::try_from(size).ok());
    let mut content = text.as_bytes().to_vec();
    content.resize(
        size.unwrap_or_else(|| panic!("a padded size: {text}")),
        b' ',
    );
    content
}

pub(crate) fn mkfifo(path: &Path) -> std::io::Result<()> {
    let found = std::env::var_os("PATH").map(|path| (OsString::from("PATH"), path));
    let spec = Spec {
        program: OsString::from("mkfifo"),
        args: vec![path.as_os_str().to_owned()],
        cwd: PathBuf::from("."),
        env: found.into_iter().collect(),
        timeout: MKFIFO_TIMEOUT,
        stdout_cap: 1 << 10,
        stderr_cap: 1 << 10,
    };
    match run(&spec) {
        Outcome::Exited { status, .. } if status.success() => Ok(()),
        outcome => Err(std::io::Error::other(format!(
            "mkfifo did not succeed: {outcome:?}"
        ))),
    }
}
