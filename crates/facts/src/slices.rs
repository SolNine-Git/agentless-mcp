use std::collections::HashSet;
use std::fmt;

use crate::values::{Symbol, SymbolKind};

const ELISION: &str = "...";
// The kinds whose header line a slice repeats above a line inside them.
const SCOPE_KINDS: [SymbolKind; 6] = [
    SymbolKind::Class,
    SymbolKind::Function,
    SymbolKind::Method,
    SymbolKind::Protocol,
    SymbolKind::Dataclass,
    SymbolKind::Enum,
];

/// The refusal of a slice whose every requested range falls outside the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutsideFile {
    pub total: u32,
    pub requested: Vec<(u32, u32)>,
}

impl fmt::Display for OutsideFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let requested: Vec<String> = self
            .requested
            .iter()
            .map(|(start, end)| format!("{start}-{end}"))
            .collect();
        write!(
            formatter,
            "no requested line range falls inside the file's {} lines: {}",
            self.total,
            requested.join(", ")
        )
    }
}

impl std::error::Error for OutsideFile {}

/// Return how many lines `text` holds, not counting a final newline.
#[must_use]
pub fn line_count(text: &str) -> u32 {
    let pieces = text.split('\n').count();
    let trailing = text.is_empty() || text.ends_with('\n');
    u32::try_from(pieces - usize::from(trailing)).unwrap_or(u32::MAX)
}

/// Return the `N| ` prefix of one numbered source line.
#[must_use]
pub fn line_prefix(number: u32) -> String {
    format!("{number}| ")
}

/// Render `content` as numbered lines within the 1-based inclusive `intervals`, or whole when
/// there are none; `...` marks each gap, and the header lines of the symbols that enclose a
/// slice stand above it.
///
/// # Errors
/// Returns `OutsideFile` when no requested range has a line inside the file.
pub fn slice_lines(
    content: &str,
    intervals: &[(u32, u32)],
    symbols: &[Symbol],
) -> Result<String, OutsideFile> {
    let lines: Vec<&str> = content.split('\n').collect();
    let total = line_count(content);
    let clipped = clamp(intervals, total)?;
    let mut rendered: Vec<String> = Vec::new();
    // Every line already shown, header or content, so that no line shows twice.
    let mut shown: HashSet<u32> = HashSet::new();
    let mut covered_to = 0;
    for (start, end) in clipped {
        if start > covered_to + 1 {
            rendered.push(ELISION.to_owned());
        }
        rendered.extend(scope_headers(&lines, symbols, start, &mut shown));
        rendered.extend((start..=end).map(|number| numbered(&lines, number)));
        shown.extend(start..=end);
        covered_to = covered_to.max(end);
    }
    if covered_to < total {
        rendered.push(ELISION.to_owned());
    }
    Ok(rendered.join("\n"))
}

// Merge 1-based inclusive intervals that overlap, sorted by start.
pub(crate) fn merge_intervals(intervals: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut ordered = intervals.to_vec();
    ordered.sort_by_key(|(start, _)| *start);
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (start, end) in ordered {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

// No intervals ask for the whole file; intervals that all clip to nothing are refused.
fn clamp(intervals: &[(u32, u32)], total: u32) -> Result<Vec<(u32, u32)>, OutsideFile> {
    if intervals.is_empty() {
        return Ok(vec![(1, total)]);
    }
    let clipped: Vec<(u32, u32)> = merge_intervals(intervals)
        .into_iter()
        .map(|(start, end)| (start.max(1), end.min(total)))
        .filter(|(low, high)| low <= high)
        .collect();
    if clipped.is_empty() {
        return Err(OutsideFile {
            total,
            requested: intervals.to_vec(),
        });
    }
    Ok(clipped)
}

// The header line of each scope symbol that opens before `start` and reaches it, outermost first;
// under the last header, `...` marks a gap down to `start`.
fn scope_headers(
    lines: &[&str],
    symbols: &[Symbol],
    start: u32,
    shown: &mut HashSet<u32>,
) -> Vec<String> {
    let mut enclosing: Vec<&Symbol> = symbols
        .iter()
        .filter(|symbol| {
            SCOPE_KINDS.contains(&symbol.kind)
                && symbol.name_line < start
                && symbol.end_line >= start
        })
        .collect();
    enclosing.sort_by_key(|symbol| symbol.name_line);
    let mut headers = Vec::new();
    let mut last_header = None;
    for symbol in enclosing {
        let header = symbol.name_line;
        if shown.contains(&header) || header as usize > lines.len() {
            continue;
        }
        shown.insert(header);
        headers.push(numbered(lines, header));
        last_header = Some(header);
    }
    if last_header.is_some_and(|header| header + 1 < start) {
        headers.push(ELISION.to_owned());
    }
    headers
}

fn numbered(lines: &[&str], number: u32) -> String {
    format!("{}{}", line_prefix(number), lines[number as usize - 1])
}
