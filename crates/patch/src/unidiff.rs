//! Reading a unified diff as one edit per hunk, and checking that a tree is the diff's base.

mod headers;
mod hunks;

use std::collections::{BTreeMap, BTreeSet};

use crate::blocks::{BlockError, Edit, ParseResult};
use crate::elision::{pad, whole_line_starts};
use crate::quoted;
use crate::trim_end;
use headers::{Headers, headers};
use hunks::{Target, end_of_hunk, hunks};

const DIFF_GIT: &str = "diff --git ";
const COMBINED: [&str; 2] = ["diff --cc ", "diff --combined "];
const OLD: &str = "--- ";
const DEV_NULL: &str = "/dev/null";
const REMEDY: &str = "point --repo at a checkout of the diff's base (for a branch, its merge-base \
                      with the target) and rerun";

/// A section that parsed but holds no text to check: a binary file or a mode-only change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffNote {
    pub path: String,
    pub reason: String,
}

/// The edits and refusals of one diff, and the sections that hold nothing to check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffParse {
    pub result: ParseResult,
    pub notes: Vec<DiffNote>,
}

/// Reads a unified diff as one edit per hunk: the search side is the hunk's context and removed
/// lines, and the replace side its context and added lines.
#[must_use]
pub fn parse_unified_diff(text: &str) -> DiffParse {
    let lines = diff_lines(text);
    let mut parsed = DiffParse::default();
    let mut at = 0;
    let mut ordinal = 0;
    let mut sections = 0;
    while let Some(line) = lines.get(at) {
        if COMBINED.iter().any(|opener| line.starts_with(opener)) {
            return refused(
                "this is a combined (merge) diff, which names two pre-images per hunk; lint the \
                 diff against one parent instead",
            );
        }
        if !opens_section(line) {
            at += 1;
            continue;
        }
        sections += 1;
        let section = file_section(&lines, at, ordinal);
        parsed.result.edits.extend(section.edits);
        parsed.result.errors.extend(section.errors);
        parsed.notes.extend(section.notes);
        ordinal = section.ordinal;
        at = section.next;
    }
    if sections == 0 {
        return refused(
            "no file sections found: this does not look like a unified diff (expected a 'diff \
             --git' or a '---' header naming a file)",
        );
    }
    parsed
}

/// Returns a refusal for each file whose edits' pre-image is not in its text in `texts`, once
/// per file. A path that `texts` does not hold is not judged.
#[must_use]
pub fn orientation(edits: &[Edit], texts: &BTreeMap<String, String>) -> Vec<BlockError> {
    let mut reported: BTreeSet<&str> = BTreeSet::new();
    let mut problems = Vec::new();
    for edit in edits {
        if reported.contains(edit.path.as_str()) {
            continue;
        }
        let Some(reason) = texts
            .get(&edit.path)
            .and_then(|text| misoriented(edit, text))
        else {
            continue;
        };
        reported.insert(&edit.path);
        problems.push(BlockError {
            index: edit.index,
            path: Some(edit.path.clone()),
            reason,
        });
    }
    problems
}

fn misoriented(edit: &Edit, text: &str) -> Option<String> {
    let path = &edit.path;
    if edit.search.is_empty() {
        return Some(format!(
            "{path} is added by this diff but already exists in --repo, so --repo is not the \
             diff's base; {REMEDY}"
        ));
    }
    if present(text, &edit.search) {
        return None;
    }
    if !edit.replace.is_empty() && present(text, &edit.replace) {
        return Some(format!(
            "{path} already contains this hunk's post-image, so the diff is already applied to \
             --repo and the checks would compare the change against itself; {REMEDY}"
        ));
    }
    Some(format!(
        "neither this hunk's pre-image nor its post-image is in {path} as --repo has it, so \
         this diff is not against --repo; {REMEDY}"
    ))
}

fn present(text: &str, block: &str) -> bool {
    whole_line_starts(&pad(text), block).next().is_some()
}

// One file's edits, refusals and notes, its last ordinal, and where the next section starts.
struct Section {
    edits: Vec<Edit>,
    errors: Vec<BlockError>,
    notes: Vec<DiffNote>,
    ordinal: i64,
    next: usize,
}

fn section_error(path: Option<String>, ordinal: i64, at: usize, reason: String) -> Section {
    Section {
        edits: Vec::new(),
        errors: vec![BlockError {
            index: ordinal,
            path,
            reason,
        }],
        notes: Vec::new(),
        ordinal: ordinal + 1,
        next: at,
    }
}

fn section_note(path: String, ordinal: i64, at: usize, reason: &str) -> Section {
    Section {
        edits: Vec::new(),
        errors: Vec::new(),
        notes: vec![DiffNote {
            path,
            reason: reason.to_owned(),
        }],
        ordinal,
        next: at,
    }
}

fn refused(reason: &str) -> DiffParse {
    let error = BlockError {
        index: 0,
        path: None,
        reason: reason.to_owned(),
    };
    DiffParse {
        result: ParseResult {
            edits: Vec::new(),
            errors: vec![error],
        },
        notes: Vec::new(),
    }
}

// The lines of the diff, without the empty line after a final line break.
fn diff_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn opens_section(line: &str) -> bool {
    line.starts_with(DIFF_GIT) || (line.starts_with(OLD) && line.len() > OLD.len())
}

fn crlf(line: &str) -> Option<String> {
    line.ends_with('\r').then(|| {
        format!(
            "a structural line ends with a carriage return, so this patch file has CRLF line \
             endings: {}; convert it to LF and rerun",
            quoted(trim_end(line))
        )
    })
}

fn file_section(lines: &[&str], at: usize, ordinal: i64) -> Section {
    let read = headers(lines, at);
    let resume = skip_to_next_section(lines, read.next, read.git_section);
    let label = label(&read);
    if let Some(reason) = read.reason {
        return section_error(label, ordinal, resume, reason);
    }
    if read.binary {
        let path = label.unwrap_or_default();
        return section_note(
            path,
            ordinal,
            resume,
            "binary, so there is no text to check",
        );
    }
    if read.renamed {
        let reason = "this section renames or copies a file, which one edit cannot express \
                      because an edit carries a single path; lint the content change separately \
                      from the rename";
        return section_error(label, ordinal, resume, reason.to_owned());
    }
    match (read.old_raw, read.new_raw) {
        (Some(old), Some(new)) => {
            paths_then_hunks(lines, (old, new), read.next, label, (ordinal, resume))
        }
        _ => without_pair(lines, &read, label, ordinal, resume),
    }
}

// A section without both of the `---` and `+++` headers.
fn without_pair(
    lines: &[&str],
    read: &Headers,
    label: Option<String>,
    ordinal: i64,
    resume: usize,
) -> Section {
    if read.old_raw.is_some() || read.new_raw.is_some() {
        let reason = "this section has only one of the '---' and '+++' headers, so which file it \
                      changes is not stated";
        return section_error(label, ordinal, resume, reason.to_owned());
    }
    if lines
        .get(read.next)
        .is_some_and(|line| line.starts_with("@@"))
    {
        let reason = "this section has hunks but neither a '---' nor a '+++' header, so the file \
                      they change is not stated; a mode-only change carries no hunks";
        return section_error(label, ordinal, resume, reason.to_owned());
    }
    let Some(label) = label else {
        let reason = "this section names no file: no '---'/'+++' pair and no 'diff --git' header";
        return section_error(None, ordinal, resume, reason.to_owned());
    };
    section_note(
        label,
        ordinal,
        resume,
        "mode change only, so there is no content to check",
    )
}

// The two paths of a section resolved into one, then its hunks.
fn paths_then_hunks(
    lines: &[&str],
    (old_raw, new_raw): (&str, &str),
    at: usize,
    label: Option<String>,
    (ordinal, resume): (i64, usize),
) -> Section {
    let old = side(old_raw, "a/");
    let new = side(new_raw, "b/");
    if let Some(reason) = old.reason.clone().or_else(|| new.reason.clone()) {
        return section_error(label, ordinal, resume, reason);
    }
    let path = match (old.path, new.path) {
        (Some(before), Some(after)) if before != after => {
            return section_error(label, ordinal, resume, two_paths(&old, &new));
        }
        (before, after) => after.or(before),
    };
    let Some(path) = path else {
        let reason = "both sides of this section are /dev/null, so it names no file at all";
        return section_error(label, ordinal, resume, reason.to_owned());
    };
    let target = Target {
        path: path.to_owned(),
        new_file: old.path.is_none(),
    };
    hunks(lines, at, &target, ordinal, resume)
}

// A section whose two sides name different files.
fn two_paths(old: &Side, new: &Side) -> String {
    let (before, after) = (old.path.unwrap_or_default(), new.path.unwrap_or_default());
    if old.raw == new.raw {
        return format!(
            "this section's '---' and '+++' headers both name {}, and only one of them carries \
             the 'a/' or 'b/' prefix git adds, so the two sides resolve to {before} and {after}; \
             regenerate the diff with git's default prefixes (drop --no-prefix)",
            old.raw
        );
    }
    format!(
        "this section renames {before} to {after}, which one edit cannot express because an \
         edit carries a single path; lint the content change separately"
    )
}

// The file that a refusal or a note names: the `+++` path, the `---` path, then the label of
// the `diff --git` line.
fn label(read: &Headers) -> Option<String> {
    [(read.new_raw, "b/"), (read.old_raw, "a/")]
        .into_iter()
        .find_map(|(raw, prefix)| side(raw?, prefix).path.map(str::to_owned))
        .or_else(|| read.named.clone())
}

// One side of a `---`/`+++` pair: its path (`None` for /dev/null), or why it has none, and its
// text without the timestamp column.
struct Side<'a> {
    path: Option<&'a str>,
    reason: Option<String>,
    raw: &'a str,
}

fn side<'a>(raw: &'a str, prefix: &str) -> Side<'a> {
    let value = raw.split('\t').next().unwrap_or(raw);
    let refusal = |reason: String| Side {
        path: None,
        reason: Some(reason),
        raw: value,
    };
    if value.starts_with('"') {
        return refusal(format!(
            "the diff names a C-quoted path ({value}); this reader does not decode those escapes, \
             because decoding one wrong attributes findings to the wrong file"
        ));
    }
    if value == DEV_NULL {
        return Side {
            path: None,
            reason: None,
            raw: value,
        };
    }
    let path = value.strip_prefix(prefix).unwrap_or(value);
    if path.is_empty() {
        return refusal(
            "the diff has an empty path on one side of its '---'/'+++' pair".to_owned(),
        );
    }
    Side {
        path: Some(path),
        reason: None,
        raw: value,
    }
}

// Where the section that holds `at` ends. A `diff --git` section runs to the next `diff --git`.
// Hunks are stepped over by their counts, so a body line that reads like a header stays inside.
fn skip_to_next_section(lines: &[&str], at: usize, git_only: bool) -> usize {
    let mut index = at;
    while let Some(line) = lines.get(index) {
        if line.starts_with(DIFF_GIT) || (!git_only && opens_section(line)) {
            return index;
        }
        let end = if line.starts_with("@@") {
            end_of_hunk(lines, index)
        } else {
            None
        };
        index = end.unwrap_or(index + 1);
    }
    lines.len()
}
