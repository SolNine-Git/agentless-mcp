//! The fixture files, and test trees built from their layouts.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::testing::{self, Tree};

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
        kind => panic!("unknown layout entry kind {kind}"),
    }
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

fn mkfifo(path: &Path) -> std::io::Result<()> {
    let status = Command::new("mkfifo").arg(path).status()?;
    if status.success() {
        return Ok(());
    }
    Err(std::io::Error::other(format!(
        "mkfifo exited with {status}"
    )))
}
