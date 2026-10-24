//! The two matching rules of the edit format: `...` elisions, and whole-line matches.

use std::collections::HashMap;

use crate::blocks::ELISION;

const ELISION_PREFIX: &str = "...\n";

/// The two sides of an edit after its `...` elisions are expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Elided {
    pub search: String,
    pub replace: String,
}

/// Where a block of whole lines occurs in a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Located {
    /// The block occurs nowhere, or the block is empty.
    Absent,
    /// The block occurs once, on these 1-based lines.
    Unique { first_line: usize, last_line: usize },
    /// The block occurs more than once, overlapping occurrences included.
    Ambiguous { matches: usize },
}

/// Expands the `...` elisions of one edit against the whole file `text`, or returns why the
/// edit cannot be expanded. A search side of only `...` anchors to the first unindented line
/// that occurs once in the file, and a leading `...` line on either side is dropped.
///
/// # Errors
///
/// Returns the refusal text when a bare `...` search has a replacement that starts with
/// whitespace or is empty, or when the file has no unique unindented line.
pub fn resolve_elisions(search: &str, replace: &str, text: &str) -> Result<Elided, &'static str> {
    let replace = without_prefix(replace);
    if search == ELISION {
        if replace.chars().next().is_none_or(text::is_space) {
            return Err(
                "elided search block needs a replacement whose first line starts at column 1",
            );
        }
        let anchor = find_anchor(text)
            .ok_or("elided search block has no unique unindented anchor line in scope")?;
        return Ok(Elided {
            search: anchor.to_owned(),
            replace: format!("{replace}\n\n{anchor}"),
        });
    }
    Ok(Elided {
        search: without_prefix(search).to_owned(),
        replace: replace.to_owned(),
    })
}

/// Returns where the lines of `block` occur in `text` as whole lines, never as a fragment of a
/// longer line.
#[must_use]
pub fn locate(text: &str, block: &str) -> Located {
    if block.is_empty() {
        return Located::Absent;
    }
    let haystack = pad(text);
    let mut starts = whole_line_starts(&haystack, block);
    match (starts.next(), starts.next()) {
        (None, _) => Located::Absent,
        (Some(first), None) => {
            let first_line = line_of(&haystack, first);
            Located::Unique {
                first_line,
                last_line: first_line + block.matches('\n').count(),
            }
        }
        (Some(_), Some(_)) => Located::Ambiguous {
            matches: 2 + starts.count(),
        },
    }
}

// Newline sentinels around the text, so that a match covers whole lines only.
pub(crate) fn pad(text: &str) -> String {
    format!("\n{text}\n")
}

// Each offset in the padded `haystack` where the padded `block` starts, overlapping ones
// included: two identical adjacent lines share the newline between them, and both count.
pub(crate) fn whole_line_starts<'a>(
    haystack: &'a str,
    block: &str,
) -> impl Iterator<Item = usize> + 'a {
    let needle = pad(block);
    let mut from = 0;
    std::iter::from_fn(move || {
        let found = from + haystack.get(from..)?.find(&needle)?;
        // A match starts at a newline, so the next byte starts a character.
        from = found + 1;
        Some(found)
    })
}

// The 1-based line of the padded text that starts after the newline at `offset`.
pub(crate) fn line_of(padded: &str, offset: usize) -> usize {
    padded[..offset].matches('\n').count() + 1
}

fn without_prefix(side: &str) -> &str {
    match side.strip_prefix(ELISION_PREFIX) {
        Some(rest) if !rest.is_empty() => rest,
        _ => side,
    }
}

// The first unindented line that occurs exactly once in the whole file.
fn find_anchor(text: &str) -> Option<&str> {
    let mut occurrences: HashMap<&str, usize> = HashMap::new();
    for line in text.split('\n') {
        *occurrences.entry(line).or_default() += 1;
    }
    text.split('\n').find(|line| {
        line.chars().next().is_some_and(|c| !text::is_space(c)) && occurrences[line] == 1
    })
}
