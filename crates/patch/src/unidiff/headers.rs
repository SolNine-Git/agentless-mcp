//! The header lines of one file section, read but not yet judged.

use super::{DIFF_GIT, OLD, crlf, opens_section};
use crate::quoted;

const NEW: &str = "+++ ";
const BINARY_LITERAL: &str = "GIT binary patch";
const BINARY_TEXT: &str = "Binary files ";
// Extended headers that say a path changed identity; the rename refusal answers them.
const RENAME_HEADERS: [&str; 4] = ["rename from ", "rename to ", "copy from ", "copy to "];
// Extended headers that carry nothing any check reads.
const IGNORED_HEADERS: [&str; 7] = [
    "index ",
    "similarity index ",
    "dissimilarity index ",
    "old mode ",
    "new mode ",
    "new file mode ",
    "deleted file mode ",
];
const SEPARATOR: &str = " b/";

// One section's header block. `git_section` says where the section ends: a `diff --git`
// section runs to the next `diff --git`, so its own `---` line is inside it.
pub(super) struct Headers<'a> {
    pub(super) named: Option<String>,
    pub(super) old_raw: Option<&'a str>,
    pub(super) new_raw: Option<&'a str>,
    pub(super) renamed: bool,
    pub(super) binary: bool,
    pub(super) reason: Option<String>,
    pub(super) next: usize,
    pub(super) git_section: bool,
}

enum Line<'a> {
    // The header block ended before this line.
    End,
    Pair(Option<&'a str>, Option<&'a str>),
    Rename,
    Ignored,
    // The header block ended after this line: a refusal, or a binary section.
    Stop(Stop),
}

enum Stop {
    Crlf(String),
    Binary,
    Unrecognised(String),
}

// Scans one section's header lines from `at`, and stops at its first hunk.
pub(super) fn headers<'a>(lines: &[&'a str], at: usize) -> Headers<'a> {
    let mut read = opening(lines[at], at);
    if read.reason.is_some() {
        return read;
    }
    while let Some(line) = lines.get(read.next) {
        match header_line(line, read.old_raw, read.new_raw) {
            Line::End => break,
            Line::Pair(old, new) => (read.old_raw, read.new_raw) = (old, new),
            Line::Rename => read.renamed = true,
            Line::Ignored => {}
            Line::Stop(stop) => return stopped(read, stop),
        }
        read.next += 1;
    }
    read
}

// The headers before the first line after the opener: a `diff --git` line names a label.
fn opening<'a>(line: &str, at: usize) -> Headers<'a> {
    let git_section = line.starts_with(DIFF_GIT);
    let mut read = Headers {
        named: None,
        old_raw: None,
        new_raw: None,
        renamed: false,
        binary: false,
        reason: None,
        next: at,
        git_section,
    };
    if git_section {
        read.reason = crlf(line);
        read.named = read.reason.is_none().then(|| diff_git_path(line)).flatten();
        read.next = at + 1;
    }
    read
}

fn stopped(read: Headers<'_>, stop: Stop) -> Headers<'_> {
    let next = read.next + 1;
    match stop {
        Stop::Crlf(reason) => Headers {
            old_raw: None,
            new_raw: None,
            renamed: false,
            reason: Some(reason),
            next,
            ..read
        },
        Stop::Binary => Headers {
            binary: true,
            next,
            ..read
        },
        Stop::Unrecognised(reason) => Headers {
            reason: Some(reason),
            next,
            ..read
        },
    }
}

fn header_line<'a>(line: &'a str, old_raw: Option<&'a str>, new_raw: Option<&'a str>) -> Line<'a> {
    if line.starts_with("@@") {
        return Line::End;
    }
    if let Some((old, new)) = path_line(line, old_raw, new_raw) {
        return crlf(line).map_or(Line::Pair(old, new), |reason| {
            Line::Stop(Stop::Crlf(reason))
        });
    }
    if line == BINARY_LITERAL || line.starts_with(BINARY_TEXT) {
        return Line::Stop(Stop::Binary);
    }
    if RENAME_HEADERS.iter().any(|header| line.starts_with(header)) {
        return Line::Rename;
    }
    if IGNORED_HEADERS
        .iter()
        .any(|header| line.starts_with(header))
    {
        return Line::Ignored;
    }
    // A mode-only or rename-only entry ends without hunks, and a binary payload ends in a blank.
    if line.is_empty() || opens_section(line) {
        return Line::End;
    }
    let reason = format!(
        "unrecognised header line in this file's diff: {}",
        quoted(line)
    );
    Line::Stop(Stop::Unrecognised(reason))
}

// The `---` and `+++` texts after this line. Only the first header of each kind is read: a second
// `---` belongs to the next section.
fn path_line<'a>(
    line: &'a str,
    old_raw: Option<&'a str>,
    new_raw: Option<&'a str>,
) -> Option<(Option<&'a str>, Option<&'a str>)> {
    if old_raw.is_none()
        && let Some(old) = line.strip_prefix(OLD)
    {
        return Some((Some(old), new_raw));
    }
    if new_raw.is_none()
        && let Some(new) = line.strip_prefix(NEW)
    {
        return Some((old_raw, Some(new)));
    }
    None
}

// The path that a plain `diff --git a/x b/x` line names. A path that holds " b/" is read only
// when a split leaves the two halves naming one file.
fn diff_git_path(line: &str) -> Option<String> {
    let rest = &line[DIFF_GIT.len()..];
    if rest.starts_with('"') {
        return None;
    }
    let at: Vec<usize> = rest.match_indices(SEPARATOR).map(|(at, _)| at).collect();
    let tail = match at[..] {
        [] => None,
        [one] => Some(&rest[one + SEPARATOR.len()..]),
        _ => agreed(rest, &at),
    };
    tail.filter(|tail| !tail.is_empty()).map(str::to_owned)
}

// At most one split agrees: two would each spell the line "a/T b/T" with a T of another length.
fn agreed<'a>(rest: &'a str, at: &[usize]) -> Option<&'a str> {
    at.iter().find_map(|&split| {
        let tail = &rest[split + SEPARATOR.len()..];
        let named = !tail.is_empty() && rest[..split].strip_prefix("a/") == Some(tail);
        named.then_some(tail)
    })
}
