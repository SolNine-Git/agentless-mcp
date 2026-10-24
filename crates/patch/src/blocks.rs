//! The SEARCH/REPLACE form: blocks split at their markers, each read into an edit or a refusal.

use serde::Serialize;

use crate::header::{HeaderPath, filename_in};
use crate::trim_end;

pub(crate) const SEARCH_MARKER: &str = "<<<<<<< SEARCH";
pub(crate) const ELISION: &str = "...";
const DIVIDER: &str = "=======";
const REPLACE_MARKER: &str = ">>>>>>> REPLACE";
const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";

/// One SEARCH/REPLACE edit: the block's position, its file, the lines to find, and the lines to
/// write in their place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Edit {
    pub index: i64,
    pub path: String,
    pub search: String,
    pub replace: String,
}

/// A block that is not an edit: its position, the path it named or inherited, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BlockError {
    pub index: i64,
    pub path: Option<String>,
    pub reason: String,
}

/// The edits of one patch text, and each block that did not read as an edit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ParseResult {
    pub edits: Vec<Edit>,
    pub errors: Vec<BlockError>,
}

impl ParseResult {
    /// Returns whether every block of the text read as an edit.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Reads `text` as SEARCH/REPLACE blocks; a block with no path of its own takes the last path
/// seen, and a text with no block is a refusal.
#[must_use]
pub fn parse_blocks(text: &str) -> ParseResult {
    let mut segments: Vec<&str> = text.split(REPLACE_MARKER).collect();
    let tail = segments.pop().unwrap_or_default();
    let mut parsed = ParseResult::default();
    let mut current: Option<String> = None;
    let mut index = 0;
    for segment in segments {
        let (path, block) = read_block(index, segment, current);
        current = path;
        match block {
            Ok(edit) => parsed.edits.push(edit),
            Err(error) => parsed.errors.push(error),
        }
        index += 1;
    }
    parsed
        .errors
        .extend(unterminated(index, tail, current.as_deref()));
    if parsed.edits.is_empty() && parsed.errors.is_empty() {
        parsed.errors.push(BlockError {
            index: 0,
            path: None,
            reason: no_blocks_reason(text),
        });
    }
    parsed
}

// One block: the path that the next block inherits, and the edit or the refusal.
fn read_block(
    index: i64,
    segment: &str,
    inherited: Option<String>,
) -> (Option<String>, Result<Edit, BlockError>) {
    let mut parts = segment.split(SEARCH_MARKER);
    let header = parts.next().unwrap_or_default();
    let path = match filename_in(header) {
        HeaderPath::Unreadable(reason) => {
            let error = BlockError {
                index,
                path: inherited.clone(),
                reason,
            };
            return (inherited, Err(error));
        }
        HeaderPath::Named(path) => Some(path.to_owned()),
        HeaderPath::Absent => inherited,
    };
    let bodies: Vec<&str> = parts.collect();
    (path.clone(), edit_of(index, path, &bodies))
}

fn edit_of(index: i64, path: Option<String>, bodies: &[&str]) -> Result<Edit, BlockError> {
    let [body] = bodies else {
        return Err(BlockError {
            index,
            path,
            reason: marker_reason(bodies.len()),
        });
    };
    let Some(path) = path else {
        return Err(BlockError {
            index,
            path: None,
            reason: "block names no file: put the path on its own line above <<<<<<< SEARCH"
                .to_owned(),
        });
    };
    match split_body(body) {
        Ok((search, replace)) => Ok(Edit {
            index,
            path,
            search,
            replace,
        }),
        Err(reason) => Err(BlockError {
            index,
            path: Some(path),
            reason,
        }),
    }
}

// The text after the last closing marker, refused when it opens a block.
fn unterminated(index: i64, tail: &str, current: Option<&str>) -> Option<BlockError> {
    let header = tail.split(SEARCH_MARKER).next()?;
    if header.len() == tail.len() {
        return None;
    }
    let path = match filename_in(header) {
        HeaderPath::Named(path) => Some(path),
        HeaderPath::Absent | HeaderPath::Unreadable(_) => current,
    };
    Some(BlockError {
        index,
        path: path.map(str::to_owned),
        reason: "block is not terminated by >>>>>>> REPLACE (truncated output?)".to_owned(),
    })
}

fn no_blocks_reason(text: &str) -> String {
    let reason = format!(
        "no SEARCH/REPLACE blocks found: expected {SEARCH_MARKER}, {DIVIDER} and \
         {REPLACE_MARKER} markers"
    );
    if text.contains(BEGIN_PATCH_MARKER) {
        return format!(
            "{reason}; this looks like {BEGIN_PATCH_MARKER} text -- rewrite each hunk as a \
             SEARCH/REPLACE block"
        );
    }
    reason
}

fn marker_reason(count: usize) -> String {
    if count == 0 {
        return "block has no <<<<<<< SEARCH marker".to_owned();
    }
    format!("block has {count} <<<<<<< SEARCH markers; expected one")
}

// The text after `<<<<<<< SEARCH`, split at its divider into the search and replace sides.
fn split_body(body: &str) -> Result<(String, String), String> {
    let lines: Vec<&str> = body.split('\n').collect();
    if let Some(reason) = crlf_reason(&lines) {
        return Err(reason.to_owned());
    }
    if !text::strip(lines[0]).is_empty() {
        return Err("text follows the <<<<<<< SEARCH marker on its own line".to_owned());
    }
    // The marker line goes, and so does the line break before the closing marker.
    let mut lines = &lines[1..];
    if let Some((&"", rest)) = lines.split_last() {
        lines = rest;
    }
    let dividers: Vec<usize> = (0..lines.len())
        .filter(|&at| is_divider(lines[at]))
        .collect();
    let [at] = dividers[..] else {
        return Err(divider_reason(dividers.len()));
    };
    let search = lines[..at].join("\n");
    let replace = lines[at + 1..].join("\n");
    if search == ELISION && text::strip(&replace) == ELISION {
        return Err("both sides of this block are '...': there is nothing to write".to_owned());
    }
    Ok((search, replace))
}

fn is_divider(line: &str) -> bool {
    trim_end(line) == DIVIDER
}

// A carriage return on the marker line or a divider means the patch text itself has CRLF ends.
fn crlf_reason(lines: &[&str]) -> Option<&'static str> {
    let mut structural = lines[..1]
        .iter()
        .chain(lines.iter().filter(|line| is_divider(line)));
    structural.any(|line| line.ends_with('\r')).then_some(
        "a structural line ends with a carriage return, so this patch text has CRLF line \
         endings; convert it to LF and rerun",
    )
}

fn divider_reason(count: usize) -> String {
    if count == 0 {
        return "block has no ======= divider between the search and replace sides".to_owned();
    }
    format!("block has {count} ======= dividers; expected one")
}
