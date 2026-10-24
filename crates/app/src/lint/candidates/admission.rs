//! Whether a candidate is checked: its parse, its edits, its paths and its pre-image, each with
//! the coverage row that says why a candidate is not checked.

use std::collections::BTreeMap;
use std::path::Path;

use checks::PairFinding;
use patch::{BlockError, DiffNote, Edit};

use super::LintCandidateInput;
use crate::patches::canonical;
use crate::rows::{coverage, refused_row};

const REPOSITORY: &str = "(repository)";

// The rows the candidate gets before any check, and the edits the checks read: none when a
// row says that the candidate is not checked.
pub(super) struct Admission {
    pub(super) rows: Vec<PairFinding>,
    pub(super) edits: Vec<Edit>,
}

// A block that did not parse costs the whole candidate, because the write side refuses such a
// patch whole. The notes of a diff come first, whatever else the candidate gets.
pub(super) fn checkable(candidate: &LintCandidateInput, root: &Path) -> Admission {
    let mut rows: Vec<PairFinding> = candidate.notes.iter().map(note_row).collect();
    let parsed = &candidate.parsed;
    if !parsed.errors.is_empty() {
        rows.extend(parsed.errors.iter().map(unparsed_row));
        return refused(rows);
    }
    if parsed.edits.is_empty() {
        let message = "not checked: the candidate contains no edits".to_owned();
        rows.push(coverage(message, "", "0 edits".to_owned()));
        return refused(rows);
    }
    match canonical(root, &parsed.edits) {
        Ok(edits) => Admission { rows, edits },
        Err(refusal) => {
            rows.push(refused_row(&refusal));
            refused(rows)
        }
    }
}

// A candidate whose pre-image is not in the tree would be compared against itself, so it is not
// checked either.
pub(super) fn oriented(admission: Admission, texts: &BTreeMap<String, String>) -> Admission {
    let misoriented = patch::orientation(&admission.edits, texts);
    if misoriented.is_empty() {
        return admission;
    }
    let mut rows = admission.rows;
    rows.extend(misoriented.iter().map(misoriented_row));
    refused(rows)
}

// What the repository read could not read, as a coverage row in the same list.
pub(super) fn unread_row(warning: &str) -> PairFinding {
    coverage(format!("not checked: {warning}"), "", warning.to_owned())
}

fn refused(rows: Vec<PairFinding>) -> Admission {
    Admission {
        rows,
        edits: Vec::new(),
    }
}

fn note_row(note: &DiffNote) -> PairFinding {
    let place = if note.path.is_empty() {
        REPOSITORY
    } else {
        &note.path
    };
    let message = format!("not checked: {place}: {}", note.reason);
    coverage(message, &note.path, note.reason.clone())
}

fn unparsed_row(error: &BlockError) -> PairFinding {
    let message = format!(
        "not checked: block {} did not parse ({})",
        error.index, error.reason
    );
    block_row(message, error)
}

fn misoriented_row(problem: &BlockError) -> PairFinding {
    block_row(format!("not checked: {}", problem.reason), problem)
}

fn block_row(message: String, error: &BlockError) -> PairFinding {
    let path = error.path.as_deref().unwrap_or_default();
    coverage(message, path, format!("block {}", error.index))
}
