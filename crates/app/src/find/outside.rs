//! The data files that spell a query outside their key outline: on a line where no outline row
//! starts, such as a key past the outline's cap, below its depth or in a list, or anywhere in a
//! data file that the size cap skipped.

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use facts::{Symbol, is_data_language};
use kernel::checkpoint;
use store::FileText;
use syntax::Language;
use text::lower;

use crate::scan::SymbolFacts;
use crate::{Error, Noted, Reading};

// The most bytes of data-file text that one find reads to look outside the outlines.
const SEARCH_BYTES: usize = 32 << 20;

/// Where data files spell a query outside their key outline, and what the search left unread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutsideOutline {
    /// Each data file that spells the query outside its outline, with the first such line, data
    /// files with an outline first, each part in scan order.
    pub places: Vec<(String, u32)>,
    /// The data files that the byte budget left unread.
    pub unread: usize,
    /// The most bytes the search reads.
    pub budget: usize,
}

// One data file the search reads, and its outline rows: none for a file the size cap skipped.
struct Target<'a> {
    path: &'a str,
    language: Language,
    rows: &'a [Symbol],
}

// What one read of a target gave: the first line outside its outline, if any, and the bytes.
enum Searched {
    Read { bytes: usize, line: Option<u32> },
    OverBudget,
}

// The data files with an outline, then the data files the scan skipped, each within the budget.
// An unreadable file is a warning.
pub(super) fn outside_outline(
    reading: &Reading<'_>,
    scan: &SymbolFacts,
    needle: &str,
) -> Noted<Result<OutsideOutline, Error>> {
    let mut found = Noted::new(OutsideOutline {
        budget: SEARCH_BYTES,
        ..OutsideOutline::default()
    });
    let mut left = SEARCH_BYTES;
    if needle.is_empty() {
        return found.map(Ok);
    }
    for target in targets(scan) {
        if let Err(stopped) = checkpoint(reading.deadline) {
            return found.map(|_| Err(stopped.into()));
        }
        match searched(&reading.repo.root, &target, needle, left) {
            Ok(Ok(Searched::Read { bytes, line })) => {
                left -= bytes;
                let place = line.map(|line| (target.path.to_owned(), line));
                found.value.places.extend(place);
            }
            Ok(Ok(Searched::OverBudget)) => found.value.unread += 1,
            Ok(Err(unreadable)) => {
                (found.diagnostics).push(format!("{}: {unreadable}", target.path));
            }
            Err(error) => return found.map(|_| Err(error)),
        }
    }
    found.map(Ok)
}

fn targets(scan: &SymbolFacts) -> impl Iterator<Item = Target<'_>> {
    let outlined = (scan.files.iter())
        .filter(|file| is_data_language(file.language))
        .map(|file| Target {
            path: &file.path,
            language: file.language,
            rows: &file.symbols,
        });
    let skipped = scan.skipped.iter().filter_map(|entry| {
        let language = Language::from_path(&entry.path).filter(|known| is_data_language(*known))?;
        Some(Target {
            path: &entry.path,
            language,
            rows: &[],
        })
    });
    outlined.chain(skipped)
}

// A path outside the repository is the error, and a file that cannot be read is the inner one.
fn searched(
    root: &Path,
    target: &Target,
    needle: &str,
    left: usize,
) -> Result<std::io::Result<Searched>, Error> {
    let absolute = io::contained_path(root, Path::new(target.path))
        .map_err(|refusal| kernel::Error::SecurityRefusal(refusal.to_string()))?;
    let bytes = match read_within(&absolute, left) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(Ok(Searched::OverBudget)),
        Err(unreadable) => return Ok(Err(unreadable)),
    };
    let text = FileText::decode(target.path.to_owned(), target.language, &bytes).text;
    Ok(Ok(Searched::Read {
        bytes: bytes.len(),
        line: first_line_outside(&text, needle, target.rows),
    }))
}

// The whole file, or `None` when it holds more bytes than are left.
fn read_within(path: &Path, left: usize) -> std::io::Result<Option<Vec<u8>>> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    if !usize::try_from(size).is_ok_and(|size| size <= left) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    let limit = u64::try_from(left).unwrap_or(u64::MAX).saturating_add(1);
    (&mut file).take(limit).read_to_end(&mut bytes)?;
    Ok((bytes.len() <= left).then_some(bytes))
}

// The first line that spells the needle, matched case-insensitively, and that no outline row
// starts on.
fn first_line_outside(text: &str, needle: &str, rows: &[Symbol]) -> Option<u32> {
    let lowered = lower(text);
    let starts: HashSet<u32> = rows.iter().map(|row| row.line).collect();
    let (mut line, mut from) = (1_u32, 0);
    for (at, _) in lowered.match_indices(needle) {
        let breaks = lowered[from..at]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        line = line.saturating_add(u32::try_from(breaks).unwrap_or(u32::MAX));
        from = at;
        if !starts.contains(&line) {
            return Some(line);
        }
    }
    None
}

#[cfg(test)]
mod tests;
