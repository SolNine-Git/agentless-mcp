//! The repository state a receipt reports: HEAD, the tree, the dirty count, and whose state it is.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::thread;

use crate::{Git, GitOutcome, Limits};

// Every call reads the receipt, so it must not hang: 5 s is far above a healthy git status, and
// the cap bounds only a pathological answer.
pub(crate) const RECEIPT: Limits = Limits {
    seconds: 5,
    shown: "5.0",
    cap: 8_000_000,
};

/// The hex digits of a short object id: HEAD and the tree in the receipt, and each history row.
// The tree id is the cache generation key, so it keeps the length the existing caches use.
pub const SHORT_SHA_LENGTH: usize = 8;

/// HEAD and tree as 8-character ids, the count of dirty paths, and a note on what is unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// HEAD's short id, or `None` when git did not print it.
    pub head_sha: Option<String>,
    /// The short id of HEAD's tree, or `None` when git did not print it.
    pub tree_oid: Option<String>,
    /// The number of `status --porcelain` lines, or `None` when git did not print them.
    pub dirty_count: Option<usize>,
    /// Each reason a value is unknown or borrowed, joined with "; ", or empty.
    pub note: String,
}

// A receipt call's stripped stdout, or the note that says why there is none.
type Read = Result<String, String>;

impl Git {
    /// Returns the resolved top level of the repository that encloses `path`, or `None`.
    #[must_use]
    pub fn root(&self, path: &Path) -> Option<PathBuf> {
        let start = if path.is_dir() { path } else { parent(path) };
        let top = self
            .receipt(start, "rev-parse", &["--show-toplevel"])
            .ok()?;
        io::resolve(Path::new(&top)).ok()
    }

    /// Reads HEAD, the tree and the dirty count of `root`, given the top level that [`Git::root`]
    /// found for `root`.
    #[must_use]
    pub fn snapshot(&self, root: &Path, enclosing: Option<&Path>) -> Snapshot {
        let Some(enclosing) = enclosing else {
            return Snapshot::outside(root);
        };
        let [head, tree, status] = self.state(root);
        let owned = io::resolve(root).is_ok_and(|resolved| resolved == enclosing);
        let borrowed = (!owned).then_some(enclosing);
        Snapshot::read(root, borrowed, head, tree, status)
    }

    // The three calls only read the repository, so they run at the same time.
    fn state(&self, root: &Path) -> [Read; 3] {
        let short = format!("--short={SHORT_SHA_LENGTH}");
        let head = || self.receipt(root, "rev-parse", &[&short, "HEAD"]);
        let tree = || self.receipt(root, "rev-parse", &[&short, "HEAD^{tree}"]);
        thread::scope(|scope| {
            let head = io::spawn_or_run(scope, &head);
            let tree = io::spawn_or_run(scope, &tree);
            let status = self.receipt(root, "status", &["--porcelain"]);
            [head.joined(), tree.joined(), status]
        })
    }

    fn receipt(&self, dir: &Path, subcommand: &'static str, rest: &[&str]) -> Read {
        let rest: Vec<&OsStr> = rest.iter().map(OsStr::new).collect();
        receipt(subcommand, self.run(dir, subcommand, &rest, RECEIPT))
    }
}

impl Snapshot {
    fn outside(root: &Path) -> Self {
        let root = root.display();
        Self {
            head_sha: None,
            tree_oid: None,
            dirty_count: None,
            note: format!(
                "{root} is not inside a git repository: HEAD and dirty count are unknown"
            ),
        }
    }

    // `borrowed` names the enclosing top level when it is not `root` itself.
    fn read(root: &Path, borrowed: Option<&Path>, head: Read, tree: Read, status: Read) -> Self {
        let borrowed_note = borrowed.map(|enclosing| {
            format!(
                "{} is not the top of its git repository: HEAD and dirty count describe {}",
                root.display(),
                enclosing.display()
            )
        });
        let failures = [&head, &tree, &status].map(|read| read.as_ref().err().cloned());
        let notes: Vec<String> = std::iter::once(borrowed_note)
            .chain(failures)
            .flatten()
            .collect();
        Self {
            head_sha: head.ok(),
            tree_oid: tree.ok(),
            dirty_count: status.ok().map(|text| dirty_count(&text)),
            note: notes.join("; "),
        }
    }
}

// Unknown, not partial: half a porcelain status would undercount a dirty tree.
fn receipt(subcommand: &str, outcome: GitOutcome) -> Read {
    match outcome {
        GitOutcome::Printed(stdout) => {
            Ok(String::from_utf8_lossy(text::strip_bytes(&stdout)).into_owned())
        }
        GitOutcome::Capped(_) => Err(format!(
            "git {subcommand} printed more than {} bytes",
            RECEIPT.cap
        )),
        GitOutcome::Failed(failure) => Err(failure.to_string()),
        GitOutcome::Unfinished(unfinished) => Err(unfinished.to_string()),
    }
}

fn dirty_count(text: &str) -> usize {
    let lines = text::splitlines(text, false);
    lines
        .iter()
        .filter(|line| !text::strip(line).is_empty())
        .count()
}

fn parent(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

#[cfg(test)]
mod tests;
