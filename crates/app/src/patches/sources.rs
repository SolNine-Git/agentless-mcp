//! The files a patch names: each read once, strictly, or the reason it was not.

use std::collections::BTreeMap;
use std::path::Path;

use patch::{ApplyResult, Edit, EditStatus};

/// The reason for a file whose bytes are not UTF-8: writing a lossy decode back would rewrite
/// every byte that does not decode.
pub const NOT_UTF8: &str =
    "not UTF-8 text: editing it would rewrite the bytes this tool cannot decode";
/// The reason for a path that names no file.
pub const NO_SUCH_FILE: &str = "no such file in this repository";

// The text of each file the edits name, and why each one that was not read was not.
#[derive(Debug, Default)]
pub(crate) struct Sources {
    pub(crate) contents: BTreeMap<String, String>,
    pub(crate) unreadable: BTreeMap<String, Unread>,
}

// Why a file was not read: the status its edits get, and the reason.
#[derive(Debug)]
pub(crate) struct Unread {
    pub(crate) status: EditStatus,
    pub(crate) reason: String,
}

// Each path resolves inside `base` before it is opened.
pub(crate) fn read_sources(base: &Path, edits: &[Edit]) -> Result<Sources, io::Refusal> {
    let mut sources = Sources::default();
    for edit in edits {
        let path = &edit.path;
        if sources.contents.contains_key(path) || sources.unreadable.contains_key(path) {
            continue;
        }
        let target = io::contained_path(base, Path::new(path))?;
        match source_text(&target) {
            Ok(text) => {
                sources.contents.insert(path.clone(), text);
            }
            Err(unread) => {
                sources.unreadable.insert(path.clone(), unread);
            }
        }
    }
    Ok(sources)
}

// A strict decode, so a write never turns an undecodable byte into U+FFFD. A stat that fails is
// a file that could not be read, never a missing one.
fn source_text(target: &Path) -> Result<String, Unread> {
    let unreadable = |reason: String| Unread {
        status: EditStatus::Unreadable,
        reason,
    };
    match io::stat_present(target) {
        Ok(Some(metadata)) if metadata.is_file() => {}
        Ok(_) => {
            return Err(Unread {
                status: EditStatus::NoSuchFile,
                reason: NO_SUCH_FILE.to_owned(),
            });
        }
        Err(error) => return Err(unreadable(io::Skipped::Unreadable(error).to_string())),
    }
    let bytes = io::read_bounded(target).map_err(|skipped| unreadable(skipped.to_string()))?;
    String::from_utf8(bytes).map_err(|_| unreadable(NOT_UTF8.to_owned()))
}

// apply_edits sees only the texts it was given, so an edit to an unread file would say "no such
// file in this patch's scope". Each such outcome gets the reason its file was not read.
pub(crate) fn with_read_reasons(
    mut result: ApplyResult,
    unreadable: &BTreeMap<String, Unread>,
) -> ApplyResult {
    for outcome in &mut result.outcomes {
        let Some(unread) = unreadable.get(&outcome.edit.path) else {
            continue;
        };
        outcome.status = unread.status;
        outcome.reason = format!("{}: {}", outcome.edit.path, unread.reason);
    }
    result
}
