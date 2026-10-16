//! The reports of check, apply and normalize, with the summary line of each receipt.

use std::collections::BTreeMap;

use patch::{ApplyResult, SyntaxVerdict};
use serde::{Serialize, Serializer};

/// One edited file's syntax verdict, or the reason it has none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCheck {
    /// The path below the repository root.
    pub path: String,
    /// The verdict, or `None` for a file that was not read.
    pub verdict: Option<SyntaxVerdict>,
    /// Why the file was not read, or empty.
    pub error: String,
}

impl FileCheck {
    /// Returns whether the file was read and parses no worse than before.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.verdict.as_ref().is_some_and(|verdict| verdict.ok)
    }
}

impl Serialize for FileCheck {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Form<'a> {
            path: &'a str,
            ok: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            verdict: Option<&'a SyntaxVerdict>,
            #[serde(skip_serializing_if = "str::is_empty")]
            error: &'a str,
        }
        Form {
            path: &self.path,
            ok: self.ok(),
            verdict: self.verdict.as_ref(),
            error: &self.error,
        }
        .serialize(serializer)
    }
}

/// What a check found: the files that were not read first, then a verdict for each changed
/// file, in path order; and the outcome of every edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckReport {
    /// The file checks.
    pub files: Vec<FileCheck>,
    /// The outcome of every edit.
    pub result: ApplyResult,
}

impl CheckReport {
    /// Returns whether every edit applied and every file parses no worse than before.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.result.ok() && self.files.iter().all(FileCheck::ok)
    }

    /// Returns the summary line of the receipt.
    #[must_use]
    pub fn summary_line(&self) -> String {
        let clean = self.files.iter().filter(|check| check.ok()).count();
        format!(
            "checked {} of {} edits across {} files; {clean} parse no worse than before",
            applied(&self.result),
            self.result.outcomes.len(),
            self.files.len()
        )
    }
}

impl Serialize for CheckReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Form<'a> {
            ok: bool,
            files: &'a [FileCheck],
            #[serde(flatten)]
            result: &'a ApplyResult,
        }
        Form {
            ok: self.ok(),
            files: &self.files,
            result: &self.result,
        }
        .serialize(serializer)
    }
}

/// The diff a patch produced, the outcome of every edit, and where it was applied.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ApplyReport {
    /// The unified diff of the tree after the write; empty when nothing was written.
    pub diff: String,
    /// Whether the working tree was written.
    pub in_place: bool,
    /// What the edits matched against: the working tree, or HEAD and its id.
    pub base: String,
    /// The outcome of every edit.
    #[serde(flatten)]
    pub result: ApplyResult,
}

impl ApplyReport {
    /// Returns whether every edit applied.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.result.ok()
    }

    /// Returns the summary line of the receipt.
    #[must_use]
    pub fn summary_line(&self) -> String {
        let place = if self.in_place {
            "the working tree"
        } else {
            "a scratch worktree"
        };
        format!(
            "applied {} of {} edits to {} files in {place}, against {}",
            applied(&self.result),
            self.result.outcomes.len(),
            self.result.new_contents.len(),
            self.base
        )
    }
}

/// The equivalence key of a patch and the key of each file it changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NormalizeReport {
    /// The key of the whole change.
    pub key: String,
    /// The key of each changed file, by path.
    pub file_keys: BTreeMap<String, String>,
    /// The outcome of every edit.
    #[serde(flatten)]
    pub result: ApplyResult,
}

impl NormalizeReport {
    /// Returns whether every edit applied, so that the key covers the whole patch.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.result.ok()
    }

    /// Returns the summary line of the receipt.
    #[must_use]
    pub fn summary_line(&self) -> String {
        format!("equivalence key over {} files", self.file_keys.len())
    }
}

fn applied(result: &ApplyResult) -> usize {
    result
        .outcomes
        .iter()
        .filter(|outcome| outcome.applied())
        .count()
}
