//! Path resolution and containment: a path is inside a root when its resolved form is.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// Why [`contained_path`] refused a candidate. It displays as the refusal text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The candidate or the root holds a NUL byte, or the working directory is unknown.
    Unusable,
    /// The candidate resolves outside the root.
    Outside {
        /// The resolved candidate.
        resolved: PathBuf,
        /// The resolved root.
        root: PathBuf,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unusable => f.write_str("path refused: not a usable filesystem path"),
            Self::Outside { resolved, root } => write!(
                f,
                "path refused: resolved to {}, which is outside the root {}",
                resolved.display(),
                root.display()
            ),
        }
    }
}

impl std::error::Error for Refusal {}

impl From<Refusal> for kernel::Error {
    fn from(refusal: Refusal) -> Self {
        Self::SecurityRefusal(refusal.to_string())
    }
}

/// Returns the absolute form of `path`: each `.` and `..` applied in order and each symlink that
/// exists followed, the last one too. A part that cannot be read is kept as written.
///
/// # Errors
///
/// Returns an error when `path` holds a NUL byte, or when `path` is relative and the working
/// directory is unknown.
pub fn resolve(path: &Path) -> io::Result<PathBuf> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        let message = "a path with a NUL byte names no file";
        return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
    }
    let mut resolved = if bytes.starts_with(b"/") {
        b"/".to_vec()
    } else {
        std::env::current_dir()?.into_os_string().into_vec()
    };
    let mut pending: Vec<Part> = Vec::new();
    push_parts(&mut pending, bytes);
    let mut seen = HashMap::new();
    while let Some(part) = pending.pop() {
        match part {
            Part::Name(name) => step(&mut resolved, &name, &mut pending, &mut seen),
            Part::Resolved(link) => {
                seen.insert(link, Some(resolved.clone()));
            }
        }
    }
    Ok(PathBuf::from(OsString::from_vec(resolved)))
}

/// Resolves `candidate` against `root`. A relative candidate is joined to the resolved root, and
/// the root itself counts as inside.
///
/// # Errors
///
/// Returns a [`Refusal`] for a path that cannot be resolved or resolves outside the root.
pub fn contained_path(root: &Path, candidate: &Path) -> Result<PathBuf, Refusal> {
    let resolved_root = resolve(root).map_err(|_| Refusal::Unusable)?;
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        resolved_root.join(candidate)
    };
    let resolved = resolve(&joined).map_err(|_| Refusal::Unusable)?;
    if resolved.starts_with(&resolved_root) {
        Ok(resolved)
    } else {
        Err(Refusal::Outside {
            resolved,
            root: resolved_root,
        })
    }
}

/// Returns true for a regular file that is not a symlink, and for a symlink whose target exists
/// and resolves inside `root`, which must be resolved.
#[must_use]
pub fn file_stays_inside(candidate: &Path, root: &Path) -> bool {
    if candidate.is_symlink() {
        return fs::canonicalize(candidate).is_ok_and(|target| target.starts_with(root));
    }
    candidate.is_file()
}

/// Returns the size of the file that `path` names, following symlinks.
///
/// # Errors
///
/// Returns the stat error.
pub fn file_size(path: &Path) -> io::Result<u64> {
    fs::metadata(path).map(|metadata| metadata.len())
}

enum Part {
    Name(Vec<u8>),
    // Marks the end of a symlink's target parts; the resolved path so far is that link's target.
    Resolved(Vec<u8>),
}

// A link maps to its resolved target, or to None while its target is still being resolved.
type Seen = HashMap<Vec<u8>, Option<Vec<u8>>>;

enum Link {
    Keep,
    Cached(Vec<u8>),
    Follow(Vec<u8>),
}

// The parts go on the stack last first, so the first part comes off first.
fn push_parts(pending: &mut Vec<Part>, path: &[u8]) {
    pending.extend(
        path.split(|&byte| byte == b'/')
            .rev()
            .map(|part| Part::Name(part.to_vec())),
    );
}

fn step(resolved: &mut Vec<u8>, name: &[u8], pending: &mut Vec<Part>, seen: &mut Seen) {
    if name.is_empty() || name == b"." {
        return;
    }
    if name == b".." {
        to_parent(resolved);
        return;
    }
    let mut next = resolved.clone();
    if next != b"/" {
        next.push(b'/');
    }
    next.extend_from_slice(name);
    match link(&next, seen) {
        Link::Keep => *resolved = next,
        Link::Cached(target) => *resolved = target,
        Link::Follow(target) => {
            if target.starts_with(b"/") {
                *resolved = b"/".to_vec();
            }
            seen.insert(next.clone(), None);
            pending.push(Part::Resolved(next));
            push_parts(pending, &target);
        }
    }
}

// The path is absolute, so it holds a slash; the parent of the root is the root.
fn to_parent(resolved: &mut Vec<u8>) {
    match resolved.iter().rposition(|&byte| byte == b'/') {
        Some(cut) if cut > 0 => resolved.truncate(cut),
        _ => resolved.truncate(1),
    }
}

// A loop, an unreadable part and a part that is not a symlink all keep the path as written.
fn link(path: &[u8], seen: &Seen) -> Link {
    let name = Path::new(OsStr::from_bytes(path));
    let is_symlink = fs::symlink_metadata(name).is_ok_and(|meta| meta.file_type().is_symlink());
    if !is_symlink {
        return Link::Keep;
    }
    match seen.get(path) {
        Some(Some(target)) => Link::Cached(target.clone()),
        Some(None) => Link::Keep,
        None => fs::read_link(name).map_or(Link::Keep, |target| {
            Link::Follow(target.into_os_string().into_vec())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{Placed, build, fixture};
    use serde_json::{Value, json};

    #[test]
    fn contained_path_cases() {
        let layout: Vec<Value> = fixture("contain", "layout");
        let tree = build(&layout);
        let root = tree.join("root");
        let all: Vec<(String, Value)> = fixture("contain", "contained_path");
        for (candidate, expected) in all {
            let candidate = tree.unplaced(&candidate);
            let found = match contained_path(&root, &candidate) {
                Ok(resolved) => json!(["inside", tree.placed(&resolved.to_string_lossy())]),
                Err(refusal) => json!(["refused", tree.placed(&refusal.to_string())]),
            };
            assert_eq!(found, expected, "candidate {candidate:?}");
        }
    }

    #[test]
    fn file_stays_inside_cases() {
        let layout: Vec<Value> = fixture("contain", "layout");
        let tree = build(&layout);
        let root = tree.join("root");
        let all: Vec<(String, bool)> = fixture("contain", "file_stays_inside");
        for (candidate, expected) in all {
            let found = file_stays_inside(&tree.join(&candidate), &root);
            assert_eq!(found, expected, "candidate {candidate}");
        }
    }

    #[test]
    fn a_relative_path_resolves_against_the_working_directory() {
        let resolved = resolve(Path::new("a/../b")).expect("the test has a working directory");
        let here = std::env::current_dir().expect("the test has a working directory");
        assert_eq!(resolved, here.join("b"));
    }

    #[test]
    fn a_refusal_becomes_a_security_refusal() {
        let error = kernel::Error::from(Refusal::Unusable);
        let text = "path refused: not a usable filesystem path".to_owned();
        assert_eq!(error, kernel::Error::SecurityRefusal(text));
    }

    #[test]
    fn file_size_follows_a_symlink() {
        let layout = vec![json!(["file", "f", "four"]), json!(["link", "l", "f"])];
        let tree = build(&layout);
        assert_eq!(file_size(&tree.join("l")).ok(), Some(4));
        assert!(file_size(&tree.join("missing")).is_err());
    }
}
