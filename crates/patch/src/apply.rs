//! Applying edits to file texts in memory, with an outcome for every edit.

use std::collections::{BTreeMap, HashMap};

use serde::ser::{Serialize, SerializeStruct, Serializer};

use crate::blocks::{ELISION, Edit};
use crate::elision::{Elided, line_of, pad, resolve_elisions, whole_line_starts};
use crate::quoted;

/// What became of one edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditStatus {
    Applied,
    NotFound,
    Ambiguous,
    NoSuchFile,
    Unreadable,
    NoAnchor,
}

impl EditStatus {
    // In the order of the variants.
    const NAMES: [&'static str; 6] = [
        "applied",
        "not_found",
        "ambiguous",
        "no_such_file",
        "unreadable",
        "no_anchor",
    ];

    /// Returns the status as the reports spell it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}

impl Serialize for EditStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// One edit's result: its status, the reason, and the match count behind an ambiguity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditOutcome {
    pub edit: Edit,
    pub status: EditStatus,
    pub reason: String,
    pub matches: usize,
}

impl EditOutcome {
    /// Returns whether this edit changed its file.
    #[must_use]
    pub fn applied(&self) -> bool {
        self.status == EditStatus::Applied
    }
}

impl Serialize for EditOutcome {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut form = serializer.serialize_struct("EditOutcome", 5)?;
        form.serialize_field("index", &self.edit.index)?;
        form.serialize_field("path", &self.edit.path)?;
        form.serialize_field("status", &self.status)?;
        form.serialize_field("reason", &self.reason)?;
        form.serialize_field("matches", &self.matches)?;
        form.end()
    }
}

/// The outcome of every edit in input order, and the new text of each file that an edit
/// changed, in the order of each file's first edit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyResult {
    pub outcomes: Vec<EditOutcome>,
    pub new_contents: Vec<(String, String)>,
}

impl ApplyResult {
    /// Returns whether every edit applied.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.outcomes.iter().all(EditOutcome::applied)
    }

    /// Returns the edits that did not apply, in input order.
    pub fn failures(&self) -> impl Iterator<Item = &EditOutcome> {
        self.outcomes.iter().filter(|outcome| !outcome.applied())
    }
}

impl Serialize for ApplyResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut changed: Vec<&str> = self
            .new_contents
            .iter()
            .map(|(path, _)| path.as_str())
            .collect();
        changed.sort_unstable();
        let applied = self
            .outcomes
            .iter()
            .filter(|outcome| outcome.applied())
            .count();
        let mut form = serializer.serialize_struct("ApplyResult", 4)?;
        form.serialize_field("applied", &applied)?;
        form.serialize_field("total", &self.outcomes.len())?;
        form.serialize_field("changed_files", &changed)?;
        form.serialize_field("outcomes", &self.outcomes)?;
        form.end()
    }
}

/// Applies `edits` to the texts in `contents`. Each edit matches against the whole file as the
/// edits before it to that file left it, and an edit to a path outside `contents` reports
/// `NoSuchFile`.
#[must_use]
pub fn apply_edits(edits: &[Edit], contents: &BTreeMap<String, String>) -> ApplyResult {
    let mut files: Vec<FileState> = Vec::new();
    let mut slots: HashMap<&str, usize> = HashMap::new();
    let mut outcomes = Vec::with_capacity(edits.len());
    for edit in edits {
        let slot = *slots.entry(&edit.path).or_insert_with(|| {
            files.push(FileState::new(&edit.path, contents.get(&edit.path)));
            files.len() - 1
        });
        let file = &mut files[slot];
        let (outcome, text) = file.apply(edit);
        if let Some(text) = text {
            file.content = text;
        }
        outcomes.push(outcome);
    }
    let new_contents = files.into_iter().filter_map(FileState::changed).collect();
    ApplyResult {
        outcomes,
        new_contents,
    }
}

// One file's text before the edits, and as the edits so far left it.
struct FileState<'a> {
    path: &'a str,
    original: Option<&'a str>,
    content: String,
}

impl<'a> FileState<'a> {
    fn new(path: &'a str, original: Option<&'a String>) -> FileState<'a> {
        let original = original.map(String::as_str);
        FileState {
            path,
            original,
            content: original.unwrap_or_default().to_owned(),
        }
    }

    // The edit's outcome, and the file's new text when the edit applied.
    fn apply(&self, edit: &Edit) -> (EditOutcome, Option<String>) {
        if self.original.is_none() {
            let reason = format!("no such file in this patch's scope: {}", edit.path);
            return (outcome(edit, EditStatus::NoSuchFile, reason, 0), None);
        }
        match placed(edit, &self.content) {
            Placed::Changed { reason, text } => {
                (outcome(edit, EditStatus::Applied, reason, 1), Some(text))
            }
            Placed::Refused {
                status,
                reason,
                matches,
            } => (outcome(edit, status, reason, matches), None),
        }
    }

    fn changed(self) -> Option<(String, String)> {
        let original = self.original?;
        (original != self.content).then(|| (self.path.to_owned(), self.content))
    }
}

fn outcome(edit: &Edit, status: EditStatus, reason: String, matches: usize) -> EditOutcome {
    EditOutcome {
        edit: edit.clone(),
        status,
        reason,
        matches,
    }
}

enum Placed {
    Changed {
        reason: String,
        text: String,
    },
    Refused {
        status: EditStatus,
        reason: String,
        matches: usize,
    },
}

fn refused(status: EditStatus, reason: String, matches: usize) -> Placed {
    Placed::Refused {
        status,
        reason,
        matches,
    }
}

// One edit against the current text: the new text, or the refusal.
fn placed(edit: &Edit, content: &str) -> Placed {
    if edit.search.is_empty() {
        // An empty search pads to a blank line, which matches the first blank line or the end.
        let reason = format!(
            "{}: the SEARCH side is empty, which anchors nowhere in a file that already exists",
            edit.path
        );
        return refused(EditStatus::NoAnchor, reason, 0);
    }
    let Elided { search, replace } = match resolve_elisions(&edit.search, &edit.replace, content) {
        Ok(elided) => elided,
        Err(reason) => return refused(EditStatus::NoAnchor, reason.to_owned(), 0),
    };
    let padded = pad(content);
    let starts: Vec<usize> = whole_line_starts(&padded, &search).collect();
    let [at] = starts[..] else {
        if starts.is_empty() {
            return refused(EditStatus::NotFound, "search text not found".to_owned(), 0);
        }
        let reason = format!("search text ambiguous ({} matches)", starts.len());
        return refused(EditStatus::Ambiguous, reason, starts.len());
    };
    // The match covers the newline before the search and the newline after it.
    let end = at + search.len() + 2;
    let updated = format!("{}{}{}", &padded[..at], pad(&replace), &padded[end..]);
    Placed::Changed {
        reason: placement(edit, &search, &padded, at),
        text: updated[1..updated.len() - 1].to_owned(),
    }
}

// A bare `...` edit names no place, so the line that the insert went above is reported.
fn placement(edit: &Edit, search: &str, padded: &str, at: usize) -> String {
    if edit.search != ELISION {
        return String::new();
    }
    format!(
        "inserted above line {}: {}",
        line_of(padded, at),
        quoted(search)
    )
}
