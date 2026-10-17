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
    let old = match working_read(&target) {
        Some(read) => match side_text(&PAIR_OLD_UNREAD, &relative, read) {
            Ok(text) => Some(text),
            Err(row) => return Proposed::Rows(vec![*row]),
        },
        None => None,
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

// The bounded read of a file that is there, and `None` for one that is not: a file the change
// creates. A stat that fails is a read that failed, never a missing file.
pub(crate) fn working_read(target: &Path) -> Option<Result<Vec<u8>, String>> {
    match io::stat_present(target) {
        Ok(None) => None,
        Ok(Some(_)) => Some(bounded_read(target)),
        Err(error) => Some(Err(io::Skipped::Unreadable(error).to_string())),
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
pub(crate) fn after_file(file: &Path, cwd: StartDir<'_>) -> Result<Vec<u8>, String> {
    match cwd {
        Ok(cwd) => bounded_read(&cwd.join(file)),
        Err(_) if file.is_absolute() => bounded_read(file),
        Err(unread) => Err(io::Skipped::Unreadable(unread.error()).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Sandbox;

    #[test]
    fn an_old_text_whose_stat_fails_is_unread_not_a_file_the_change_creates() {
        let sandbox = Sandbox::new();
        let root = sandbox.repo("repo", &[("secret/x.py", "def f():\n    return 1\n")]);
        if !sandbox.modes_bind() {
            return;
        }
        let after = After::Text("def f():\n    return 2\n".to_owned());
        sandbox.chmod("repo/secret", 0o000);
        let found = proposed(&root, "secret/x.py", &after, Ok(&root));
        sandbox.chmod("repo/secret", 0o755);
        let Proposed::Rows(rows) = found else {
            panic!("the old text was not refused");
        };
        let shown: Vec<(&str, &str)> = rows
            .iter()
            .map(|row| (row.finding.message.as_str(), row.finding.evidence.as_str()))
            .collect();
        let message = "not checked: the text of secret/x.py before the change was not read \
                       (unreadable: Permission denied), so no pair check compared this file.";
        assert_eq!(shown, [(message, "unreadable: Permission denied")]);
    }
}
