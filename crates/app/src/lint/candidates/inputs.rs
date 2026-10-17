//! The candidate inputs: one patch file or a directory of them, and one unified diff.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use text::{CodePointString, split_suffix};

use super::LintCandidateInput;
use crate::patches::load_edits;
use crate::proposed::after_file;
use crate::repo::{below_cwd, expand_user};
use crate::rows::bounded_read;
use crate::{Error, Services};

/// Reads `target`, a patch file or a directory that holds one patch file per candidate, each in
/// the edits.json form or as SEARCH/REPLACE blocks. A leading `~` is the home directory, and a
/// relative path is below the start-up directory. A directory's files come in code point order,
/// and each file's stem is its candidate's id.
///
/// # Errors
///
/// Returns an input-unreadable error for a target that is neither a file nor a directory and for
/// a file or directory that cannot be read, and an operation failure for an empty directory or a
/// file that is not a valid edits document.
pub fn load_lint_candidates(
    services: &Services,
    target: &Path,
) -> Result<Vec<LintCandidateInput>, Error> {
    let expanded = expand_user(target, services.home.as_deref());
    let resolved = io::resolve(&below_cwd(services, &expanded)?)
        .map_err(|error| unreadable(io::strerror(&error)))?;
    if resolved.is_dir() {
        let files = patch_files(&resolved)?;
        if files.is_empty() {
            let message = format!(
                "no patch files in {}: one file per candidate patch",
                resolved.display()
            );
            return Err(kernel::Error::OperationFailed(message).into());
        }
        return files.iter().map(PathBuf::as_path).map(candidate).collect();
    }
    if !resolved.is_file() {
        let message = format!(
            "{} is neither a patch file nor a directory of them",
            resolved.display()
        );
        return Err(unreadable(message));
    }
    Ok(vec![candidate(&resolved)?])
}

/// Reads `target` as one unified diff, a candidate whose id is the file's stem. A relative path
/// is below the start-up directory.
///
/// # Errors
///
/// Returns an input-unreadable error for a file that cannot be read.
pub fn load_lint_diff(services: &Services, target: &Path) -> Result<LintCandidateInput, Error> {
    let read = after_file(target, services.cwd.as_deref());
    let parsed = patch::parse_unified_diff(&text_of(read, target, "diff")?);
    Ok(LintCandidateInput {
        id: stem(target),
        parsed: parsed.result,
        notes: parsed.notes,
    })
}

fn candidate(path: &Path) -> Result<LintCandidateInput, Error> {
    let text = text_of(bounded_read(path), path, "candidate")?;
    Ok(LintCandidateInput {
        id: stem(path),
        parsed: load_edits(&text)?,
        notes: Vec::new(),
    })
}

// The bounded read of an input, decoded with U+FFFD, or its refusal.
fn text_of(read: Result<Vec<u8>, String>, path: &Path, what: &str) -> Result<String, Error> {
    match read {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(reason) => Err(unreadable(format!(
            "cannot read {what} {}: {reason}",
            name(path)
        ))),
    }
}

// Every file of `directory`, symlinks to files included, in the code point order of the names.
pub(crate) fn patch_files(directory: &Path) -> Result<Vec<PathBuf>, Error> {
    let listing_error = |error: std::io::Error| {
        let message = format!(
            "cannot read {}: {}",
            directory.display(),
            io::strerror(&error)
        );
        unreadable(message)
    };
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(listing_error)? {
        let path = entry.map_err(listing_error)?.path();
        if path.is_file() {
            files.push(path);
        }
    }
    files.sort_by_cached_key(|path| code_points(path.file_name().unwrap_or_default()));
    Ok(files)
}

fn code_points(name: &OsStr) -> CodePointString {
    CodePointString::from_utf8_escaped(name.as_bytes())
}

// The last component of the path, ".." included, and empty for a path that has none.
pub(crate) fn name(path: &Path) -> String {
    match path.components().next_back() {
        Some(Component::Normal(name)) => name.to_string_lossy().into_owned(),
        Some(Component::ParentDir) => "..".to_owned(),
        _ => String::new(),
    }
}

// The name without its last suffix.
pub(crate) fn stem(path: &Path) -> String {
    split_suffix(&name(path)).0.to_owned()
}

fn unreadable(message: String) -> Error {
    kernel::Error::InputUnreadable(message).into()
}
