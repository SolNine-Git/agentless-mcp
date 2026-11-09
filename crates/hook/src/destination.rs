//! Where the call writes: the tier-1 rule, the repository root, the path below it, and the old
//! text.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use git::Git;

use crate::halt::Halt;
use crate::repository::{self, NoRoot};
use crate::text::{grouped, shown};

// A path with a NUL byte names no file. The reason names lstat, the first call that refuses it.
const EMBEDDED_NUL: &str = "ValueError: lstat: embedded null character in path";

pub(crate) fn resolve(given: &Path) -> Result<PathBuf, Halt> {
    io::resolve(given).map_err(|error| unresolved(&error))
}

fn unresolved(error: &std::io::Error) -> Halt {
    if error.kind() == ErrorKind::InvalidInput {
        Halt::internal(EMBEDDED_NUL)
    } else {
        Halt::internal(io::os_error_line(error))
    }
}

pub(crate) fn require_tier1(resolved: &Path) -> Result<(), Halt> {
    let Some(name) = resolved.file_name() else {
        return Err(not_tier1(""));
    };
    if name.to_str().and_then(checks::pair_language).is_some() {
        return Ok(());
    }
    Err(not_tier1(&name.to_string_lossy()))
}

fn not_tier1(name: &str) -> Halt {
    Halt::skipped(format!("not a tier-1 source file: {name}"))
}

// The root comes from where the caller writes, and containment is tested on the resolved file.
pub(crate) fn root(git: &Git, given: &Path, budget: Duration) -> Result<PathBuf, Halt> {
    repository::top_level(git, &anchor(given), budget).map_err(halt)
}

// The halt that each way of naming no root gives.
fn halt(no_root: NoRoot) -> Halt {
    match no_root {
        NoRoot::Outside => Halt::skipped("not in a git repository"),
        NoRoot::Git(reason) => Halt::degraded(reason),
        NoRoot::NotUtf8(text) => Halt::internal(format!("UnicodeDecodeError: {text}")),
        NoRoot::Unresolved(error) => unresolved(&error),
    }
}

// The nearest existing directory above the target. A fixed point ends the climb.
fn anchor(given: &Path) -> PathBuf {
    let mut anchor = parent(given);
    while !anchor.is_dir() {
        let up = parent(&anchor);
        if up == anchor {
            break;
        }
        anchor = up;
    }
    anchor
}

fn parent(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => PathBuf::from("."),
        Some(parent) => parent.to_path_buf(),
        None => path.to_path_buf(),
    }
}

pub(crate) fn relative(resolved: &Path, root: &Path) -> Result<String, Halt> {
    let Ok(rel) = resolved.strip_prefix(root) else {
        return Err(Halt::skipped(
            "the resolved target is outside the repository",
        ));
    };
    if rel.as_os_str().is_empty() {
        return Ok(".".to_owned());
    }
    rel.to_str().map(str::to_owned).ok_or_else(|| {
        let rel = shown(rel);
        Halt::degraded(format!("lint failed: not a UTF-8 path: {rel}"))
    })
}

// The text a Write replaces or an Edit changes, or None when no file is there.
pub(crate) fn old_text(resolved: &Path) -> Result<Option<String>, Halt> {
    let metadata = match fs::metadata(resolved) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            let shown = io::os_error_text(&error, resolved);
            return Err(Halt::degraded(format!("cannot stat the target: {shown}")));
        }
    };
    if !metadata.is_file() {
        return Err(Halt::skipped("the target is not a regular file"));
    }
    let bytes = io::read_bounded(resolved).map_err(|skipped| match skipped {
        io::Skipped::TooLarge { .. } => {
            let cap = grouped(io::MAX_FILE_BYTES);
            Halt::degraded(format!("the old text is over {cap} bytes"))
        }
        io::Skipped::Unreadable(error) => {
            let shown = io::os_error_text(&error, resolved);
            Halt::degraded(format!("cannot read the old text: {shown}"))
        }
    })?;
    io::decode_strict(bytes).map(Some).map_err(|error| {
        let start = error.start;
        Halt::degraded(format!("the old text is not UTF-8: byte {start}"))
    })
}

#[cfg(test)]
mod tests;
