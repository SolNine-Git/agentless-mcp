//! The history's two forms: the span header, one row per commit with its body, and the notes
//! on what the answer cannot vouch for in the text, and every commit in the JSON.

use app::{HistoryEntry, HistoryResult, history_entry_text};
use prompts::messages::{
    HISTORY_DIRTY_FILE, HISTORY_DIRTY_UNKNOWN, HISTORY_HEADER, HISTORY_MORE_COMMITS,
    HISTORY_OUTPUT_CAPPED, HISTORY_SEATS_CAPPED, HISTORY_SHALLOW, HISTORY_SHALLOW_UNKNOWN,
    HISTORY_SIGNATURE_CHANGED,
};
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use text::one_line;

use crate::envelope::{Listing, wrap_json};
use crate::{Rendered, View};

impl View for HistoryResult {
    fn rendered(&self) -> Rendered<'_> {
        Rendered::new(history_text(self), HistoryJson::of(self), None, wrap_json)
    }
}

fn history_text(result: &HistoryResult) -> String {
    let mut text = span_header(result) + "\n";
    for entry in &result.entries {
        text.push_str(&history_entry_text(entry));
    }
    for note in notes(result) {
        text.push_str(&note);
        text.push('\n');
    }
    text
}

fn span_header(result: &HistoryResult) -> String {
    HISTORY_HEADER.render((
        &one_line(&result.path),
        &one_line(&result.target),
        result.start_line as usize,
        result.end_line as usize,
        result.entries.len(),
    ))
}

// Each note names a reason to doubt the list: a cap, a working-tree edit, a moved signature, a
// shallow clone.
fn notes(result: &HistoryResult) -> Vec<String> {
    let count = result.entries.len();
    let path = one_line(&result.path);
    let start = result.committed_start_line as usize;
    let end = result.committed_end_line as usize;
    let mut notes = Vec::new();
    if result.output_capped {
        notes.push(HISTORY_OUTPUT_CAPPED.render((count, git::MAX_HISTORY_OUTPUT_BYTES)));
    }
    if result.list_end.seats_capped() {
        notes.push(HISTORY_SEATS_CAPPED.render((&path, start, end, count)));
    } else if result.list_end.more_commits() {
        notes.push(HISTORY_MORE_COMMITS.render(count));
    }
    match result.dirty {
        None => {
            let note = one_line(&result.dirty_note);
            notes.push(HISTORY_DIRTY_UNKNOWN.render((&path, start, end, &note)));
        }
        Some(true) => notes.push(HISTORY_DIRTY_FILE.render((&path, start, end))),
        Some(false) => {}
    }
    notes.extend(state_notes(result));
    notes
}

// The notes on the symbol's signature and on the clone.
fn state_notes(result: &HistoryResult) -> Vec<String> {
    let mut notes = Vec::new();
    if result.signature_changed {
        let name =
            (result.target.rsplit_once("::")).map_or(result.target.as_str(), |(_, name)| name);
        notes.push(HISTORY_SIGNATURE_CHANGED.render(&one_line(name)));
    }
    match result.shallow {
        None => notes.push(HISTORY_SHALLOW_UNKNOWN.render(&one_line(&result.shallow_note))),
        Some(true) => notes.push(HISTORY_SHALLOW.to_owned()),
        Some(false) => {}
    }
    notes
}

// The span, the commits that the ceiling may trim, then the flags and notes of the result.
#[derive(Clone)]
struct HistoryJson<'a> {
    result: &'a HistoryResult,
    commits: Vec<CommitJson<'a>>,
}

impl Serialize for HistoryJson<'_> {
    // The committed span appears only when it differs from the working-tree span.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let result = self.result;
        let committed = (result.committed_start_line, result.committed_end_line);
        let moved = committed != (result.start_line, result.end_line);
        let mut fields = serializer.serialize_struct("History", 11 + 2 * usize::from(moved))?;
        fields.serialize_field("target", &result.target)?;
        fields.serialize_field("path", &result.path)?;
        fields.serialize_field("start_line", &result.start_line)?;
        fields.serialize_field("end_line", &result.end_line)?;
        fields.serialize_field("commits", &self.commits)?;
        fields.serialize_field("more_commits", &result.list_end.more_commits())?;
        fields.serialize_field("output_capped", &result.output_capped)?;
        fields.serialize_field("dirty", &result.dirty)?;
        fields.serialize_field("seats_capped", &result.list_end.seats_capped())?;
        fields.serialize_field("shallow", &result.shallow)?;
        fields.serialize_field("signature_changed", &result.signature_changed)?;
        if moved {
            fields.serialize_field("committed_start_line", &committed.0)?;
            fields.serialize_field("committed_end_line", &committed.1)?;
        }
        fields.end()
    }
}

#[derive(Clone, Serialize)]
struct CommitJson<'a> {
    sha: &'a str,
    authored: &'a str,
    subject: &'a str,
    body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    body_truncated: Option<BodyCut>,
}

#[derive(Clone, Copy, Serialize)]
struct BodyCut {
    lines_shown: usize,
    lines: usize,
}

impl<'a> HistoryJson<'a> {
    fn of(result: &'a HistoryResult) -> Self {
        HistoryJson {
            result,
            commits: result.entries.iter().map(CommitJson::of).collect(),
        }
    }
}

impl<'a> CommitJson<'a> {
    fn of(entry: &'a HistoryEntry) -> Self {
        let cut = entry.body_shown() < entry.body_total;
        CommitJson {
            sha: &entry.sha,
            authored: &entry.authored,
            subject: &entry.subject,
            body: entry.body_lines.join("\n"),
            body_truncated: cut.then_some(BodyCut {
                lines_shown: entry.body_shown(),
                lines: entry.body_total,
            }),
        }
    }
}

impl Listing for HistoryJson<'_> {
    fn items(&self) -> usize {
        self.commits.len()
    }

    fn prefix(&self, kept: usize) -> Self {
        let mut prefix = self.clone();
        prefix.commits.truncate(kept);
        prefix
    }
}
