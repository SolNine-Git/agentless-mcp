//! The files the working tree changed since a commit: the git calls that list them, and their
//! parsers.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;

use text::CodePointString;

use crate::{Git, GitOutcome, Limits};

/// The stdout cap of the change listings; a listing cut short would hide a changed file.
pub const MAX_LISTING_BYTES: usize = 8_000_000;
/// The stdout cap of [`Git::verify`], which prints one object name.
pub const MAX_COMMIT_OUTPUT_BYTES: usize = 4_096;
/// The stdout cap of [`Git::blob`]: the per-file cap.
pub const MAX_BLOB_OUTPUT_BYTES: usize = io::MAX_FILE_BYTES;
/// The leading bytes that [`looks_binary`] searches for a NUL byte: git's own binary rule.
pub const BINARY_PROBE_BYTES: usize = 8_000;
const UNTRACKED: char = '?';
/// The statuses whose two sides the review compares.
pub const COMPARED_STATUSES: [char; 7] = ['A', 'C', 'D', 'M', 'R', 'T', UNTRACKED];

// Rename detection compares every added file with every deleted one, and the untracked listing
// walks the whole tree, so both get more time than a receipt call.
pub(crate) const LISTING: Limits = Limits {
    seconds: 30,
    shown: "30.0",
    cap: MAX_LISTING_BYTES,
};
// rev-parse prints one object name, so this cap bounds only a git that prints something else.
pub(crate) const COMMIT: Limits = Limits {
    seconds: 5,
    shown: "5.0",
    cap: MAX_COMMIT_OUTPUT_BYTES,
};
pub(crate) const BLOB: Limits = Limits {
    seconds: 5,
    shown: "5.0",
    cap: MAX_BLOB_OUTPUT_BYTES,
};

const TWO_PATH_STATUSES: [char; 2] = ['R', 'C'];
const MODE_SHOWN_CHARS: usize = 12;

/// What one side of a change is at its path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    /// No entry.
    Absent,
    /// A regular file, executable or not.
    File,
    /// A symbolic link.
    Link,
    /// A submodule commit.
    Gitlink,
}

/// One path that differs from the commit: git's status letter, both paths, and both sides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The first character of git's status.
    pub status: char,
    /// The path in the working tree.
    pub path: String,
    /// The path in the commit; it differs from `path` for a rename or a copy.
    pub old_path: String,
    /// What the commit has at `old_path`.
    pub old: Side,
    /// What the working tree has at `path`.
    pub new: Side,
}

/// Git output the parsers cannot read. It displays as the detail of the refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Malformed {
    /// A diff record does not open with its metadata field.
    NoMeta,
    /// A diff record ends before its path.
    NoPath,
    /// A diff record carries a mode that names no file type.
    Mode(String),
    /// `rev-parse` printed something other than one full object name.
    NotOneObjectName,
}

impl Git {
    /// Resolves `revision` to one commit with `rev-parse --verify --end-of-options`.
    #[must_use]
    pub fn verify(&self, root: &Path, revision: &str) -> GitOutcome {
        let target = OsString::from(format!("{revision}^{{commit}}"));
        let rest = [
            OsStr::new("--verify"),
            OsStr::new("--end-of-options"),
            &target,
        ];
        self.run(root, "rev-parse", &rest, COMMIT)
    }

    /// Lists each path the working tree changed since `commit` as `diff --raw -z` records.
    #[must_use]
    pub fn changes(&self, root: &Path, commit: &str) -> GitOutcome {
        // --raw carries both modes, which name a symlink or a submodule. --relative keeps every
        // path relative to -C when the root is below the git top level.
        let rest = ["--raw", "-z", "-M", "--relative", commit, "--"].map(OsStr::new);
        self.run(root, "diff", &rest, LISTING)
    }

    /// Lists the untracked files that no ignore rule excludes, NUL-separated.
    #[must_use]
    pub fn untracked(&self, root: &Path) -> GitOutcome {
        let rest = ["--others", "--exclude-standard", "-z"].map(OsStr::new);
        self.run(root, "ls-files", &rest, LISTING)
    }

    /// Prints `path` as `commit` has it, up to the per-file cap.
    #[must_use]
    pub fn blob(&self, root: &Path, commit: &str, path: &str) -> GitOutcome {
        // Plumbing, so no textconv driver runs. "./" reads the path from -C, not from the top.
        let object = OsString::from(format!("{commit}:./{path}"));
        self.run(root, "cat-file", &[OsStr::new("blob"), &object], BLOB)
    }
}

/// Parses `git diff --raw -z` output into changes, in git's order.
///
/// # Errors
///
/// Returns [`Malformed`] for the first record that the parser cannot read.
pub fn parse_changes(raw: &[u8]) -> Result<Vec<Change>, Malformed> {
    let mut fields = fields(raw);
    let mut changes = Vec::new();
    while let Some(meta) = fields.next() {
        changes.push(record(meta, &mut fields)?);
    }
    Ok(changes)
}

/// Parses `git ls-files --others -z` output; a name that ends in "/" is a nested repository.
#[must_use]
pub fn parse_untracked(raw: &[u8]) -> Vec<Change> {
    fields(raw)
        .map(|field| untracked(&decoded(field)))
        .collect()
}

/// Returns the one full object name that `rev-parse` printed.
///
/// # Errors
///
/// Returns [`Malformed::NotOneObjectName`] when the stripped text is not 40 or 64 hex digits.
pub fn parse_commit(stdout: &[u8]) -> Result<String, Malformed> {
    let text = String::from_utf8_lossy(stdout);
    let name = text::strip(&text);
    let hex = name
        .bytes()
        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    if hex && matches!(name.len(), 40 | 64) {
        return Ok(name.to_owned());
    }
    Err(Malformed::NotOneObjectName)
}

/// Returns whether `data` holds a NUL byte in its first [`BINARY_PROBE_BYTES`] bytes.
#[must_use]
pub fn looks_binary(data: &[u8]) -> bool {
    data[..data.len().min(BINARY_PROBE_BYTES)].contains(&0)
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoMeta => f.write_str(
                "a diff record does not open with ':<mode> <mode> <sha> <sha> <status>'",
            ),
            Self::NoPath => f.write_str("a diff record ends before its path"),
            Self::Mode(mode) => {
                let shown: String = mode.chars().take(MODE_SHOWN_CHARS).collect();
                let quoted = CodePointString::from(shown.as_str()).quoted();
                write!(
                    f,
                    "a diff record carries the mode {quoted}, which names no file type"
                )
            }
            Self::NotOneObjectName => f.write_str("rev-parse did not print one full object name"),
        }
    }
}

// A final NUL ends the last field. It does not open an empty one.
fn fields(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    let body = raw.strip_suffix(b"\0").unwrap_or(raw);
    let split = (!raw.is_empty()).then(|| body.split(|&byte| byte == 0));
    split.into_iter().flatten()
}

fn record<'a>(
    meta: &[u8],
    fields: &mut impl Iterator<Item = &'a [u8]>,
) -> Result<Change, Malformed> {
    let (old_mode, new_mode, status) = meta_parts(&decoded(meta))?;
    let old_path = next_path(fields)?;
    let path = if TWO_PATH_STATUSES.contains(&status) {
        next_path(fields)?
    } else {
        old_path.clone()
    };
    // A copy leaves its source in place, so the copy's old side is absent.
    let old = if status == 'C' {
        Side::Absent
    } else {
        side(&old_mode)?
    };
    let new = side(&new_mode)?;
    Ok(Change {
        status,
        path,
        old_path,
        old,
        new,
    })
}

// The metadata field is ":<old mode> <new mode> <old sha> <new sha> <status>".
fn meta_parts(text: &str) -> Result<(String, String, char), Malformed> {
    let Some(rest) = text.strip_prefix(':') else {
        return Err(Malformed::NoMeta);
    };
    let parts: Vec<&str> = rest.split(' ').collect();
    let [old, new, _, _, status] = parts[..] else {
        return Err(Malformed::NoMeta);
    };
    let status = status.chars().next().ok_or(Malformed::NoMeta)?;
    Ok((old.to_owned(), new.to_owned(), status))
}

fn next_path<'a>(fields: &mut impl Iterator<Item = &'a [u8]>) -> Result<String, Malformed> {
    fields.next().map(decoded).ok_or(Malformed::NoPath)
}

fn side(mode: &str) -> Result<Side, Malformed> {
    match mode {
        "000000" => Ok(Side::Absent),
        "100644" | "100755" => Ok(Side::File),
        "120000" => Ok(Side::Link),
        "160000" => Ok(Side::Gitlink),
        _ => Err(Malformed::Mode(mode.to_owned())),
    }
}

fn untracked(name: &str) -> Change {
    let (path, new) = if name.ends_with('/') {
        (name.trim_end_matches('/'), Side::Gitlink)
    } else {
        (name, Side::File)
    };
    Change {
        status: UNTRACKED,
        path: path.to_owned(),
        old_path: path.to_owned(),
        old: Side::Absent,
        new,
    }
}

// A name that is not UTF-8 decodes with replacement characters, so reading it fails and becomes a
// row rather than naming some other file.
fn decoded(field: &[u8]) -> String {
    String::from_utf8_lossy(field).into_owned()
}

#[cfg(test)]
mod tests;
