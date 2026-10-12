//! The proposed form: one repository file and the whole new text proposed for it.

use std::path::{Path, PathBuf};

use checks::{PairFinding, TextPair};
use prompts::messages::{PAIR_NEW_UNREAD, PAIR_OLD_UNREAD};

use crate::UnreadCwd;
use crate::rows::{Gap, bounded_read, refused_row, side_row, side_text};

type StartDir<'a> = Result<&'a Path, &'a UnreadCwd>;

/// Where the proposed whole new text of a file comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum After {
    /// A file that holds the text. A relative path is below the start-up directory.
    File(PathBuf),
    /// The text itself, as the hook holds it.
    Text(String),
}

// What the proposed form compares: one pair, coverage rows alone, or nothing for a file that no
// pair check reads.
pub(crate) enum Proposed {
    Pair(TextPair),
    Rows(Vec<PairFinding>),
    Skipped,
}

// The language gate comes before any read, so a file no check reads costs no read.
pub(crate) fn proposed(root: &Path, path: &str, after: &After, cwd: StartDir<'_>) -> Proposed {
    let target = match io::contained_path(root, Path::new(path)) {
        Ok(target) => target,
        Err(refusal) => return Proposed::Rows(vec![refused_row(&refusal)]),
    };
    let Some(relative) = relative_path(root, &target) else {
        return Proposed::Skipped;
    };
    if checks::pair_language(&relative).is_none() {
        return Proposed::Skipped;
    }
    // A missing file is one the change creates, which differs from one that could not be read.
    let old = if target.exists() {
        match side_text(&PAIR_OLD_UNREAD, &relative, bounded_read(&target)) {
            Ok(text) => Some(text),
            Err(row) => return Proposed::Rows(vec![*row]),
        }
    } else {
        None
    };
    match new_text(&relative, after, cwd) {
        Ok(new) => Proposed::Pair(TextPair {
            path: relative,
            old,
            new: Some(new),
        }),
        Err(row) => Proposed::Rows(vec![*row]),
    }
}

// A name that is not UTF-8 shows U+FFFD in the report, and the pair still reads the real file.
fn relative_path(root: &Path, target: &Path) -> Option<String> {
    let relative = target.strip_prefix(root).ok()?;
    Some(relative.to_string_lossy().into_owned())
}

// A text in memory meets the size cap that a file of the same bytes meets.
fn new_text(path: &str, after: &After, cwd: StartDir<'_>) -> Result<String, Gap> {
    match after {
        After::File(file) => side_text(&PAIR_NEW_UNREAD, path, after_file(file, cwd)),
        After::Text(text) if text.len() > io::MAX_FILE_BYTES => {
            let size = u64::try_from(text.len()).unwrap_or(u64::MAX);
            let reason = io::Skipped::TooLarge { size }.to_string();
            Err(Box::new(side_row(&PAIR_NEW_UNREAD, path, reason)))
        }
        After::Text(text) => Ok(text.clone()),
    }
}

// A relative file is below the start-up directory, so an unread directory leaves it unread.
fn after_file(file: &Path, cwd: StartDir<'_>) -> Result<Vec<u8>, String> {
    match cwd {
        Ok(cwd) => bounded_read(&cwd.join(file)),
        Err(_) if file.is_absolute() => bounded_read(file),
        Err(unread) => Err(io::Skipped::Unreadable(unread.error()).to_string()),
    }
}
