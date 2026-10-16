//! The hunks of one file section, each consumed by the line counts its `@@` header declares.

use super::{Section, crlf, opens_section, section_error};
use crate::blocks::{BlockError, Edit};
use crate::natural::{Natural, leading};
use crate::{quoted, trim_end};

const NO_NEWLINE: &str = "\\ ";
// `git format-patch` writes this before its version, after the last hunk.
const TRAILER: &str = "--";
const OLD: usize = 0;
const NEW: usize = 1;

// The file that one section's hunks change, and whether the diff creates it.
pub(super) struct Target {
    pub(super) path: String,
    pub(super) new_file: bool,
}

// What one `@@ -start,count +start,count @@` header declares.
struct Counts {
    old_start: Natural,
    old_count: Natural,
    new_count: Natural,
}

// Reads every hunk of one section into one edit each, or up to the first refusal.
pub(super) fn hunks(
    lines: &[&str],
    at: usize,
    target: &Target,
    ordinal: i64,
    resume: usize,
) -> Section {
    let path = &target.path;
    if !opens_hunk(lines, at) {
        let reason =
            format!("{path} has a file header but no hunks, so there is no change to read");
        return section_error(Some(path.clone()), ordinal, resume, reason);
    }
    let mut edits = Vec::new();
    let (mut next, mut index, mut floor) = (at, ordinal, Natural::default());
    let mut header = lines[at];
    while opens_hunk(lines, next) {
        header = lines[next];
        match hunk(lines, next, target, index, &floor) {
            Ok((edit, after, raised)) => {
                edits.push(edit);
                (next, floor) = (after, raised);
            }
            Err(refusal) => return refused_section(edits, refusal, resume),
        }
        index += 1;
    }
    if unread_content(lines, next, resume) {
        // The last header read is the one whose counts ended the hunk before its body did.
        let refusal = BlockError {
            index,
            path: Some(path.clone()),
            reason: count_mismatch(path, header),
        };
        return refused_section(edits, refusal, resume);
    }
    Section {
        edits,
        errors: Vec::new(),
        notes: Vec::new(),
        ordinal: index,
        next,
    }
}

fn refused_section(edits: Vec<Edit>, refusal: BlockError, resume: usize) -> Section {
    Section {
        edits,
        ordinal: refusal.index + 1,
        errors: vec![refusal],
        notes: Vec::new(),
        next: resume,
    }
}

fn opens_hunk(lines: &[&str], at: usize) -> bool {
    lines.get(at).is_some_and(|line| line.starts_with("@@"))
}

// One hunk as an edit, the line after it, and the first pre-image line the next hunk may claim.
fn hunk(
    lines: &[&str],
    at: usize,
    target: &Target,
    index: i64,
    floor: &Natural,
) -> Result<(Edit, usize, Natural), BlockError> {
    let header = lines[at];
    let path = &target.path;
    let refusal = |reason| BlockError {
        index,
        path: Some(path.clone()),
        reason,
    };
    let counts = hunk_counts(header, target, floor).map_err(refusal)?;
    let body = hunk_body(
        lines,
        at + 1,
        [&counts.old_count, &counts.new_count],
        path,
        header,
    )
    .map_err(refusal)?;
    if lines.get(body.next).is_some_and(|line| is_stray_body(line)) {
        return Err(refusal(count_mismatch(path, header)));
    }
    let edit = Edit {
        index,
        path: path.clone(),
        search: body.search,
        replace: body.replace,
    };
    Ok((edit, body.next, counts.old_start.plus(&counts.old_count)))
}

fn hunk_counts(header: &str, target: &Target, floor: &Natural) -> Result<Counts, String> {
    if let Some(reason) = crlf(header) {
        return Err(reason);
    }
    let path = &target.path;
    let Some(counts) = parse_header(header) else {
        return Err(format!(
            "{path} has a hunk header that is not '@@ -old,count +new,count @@': {}",
            quoted(header)
        ));
    };
    let shown = text::strip(header);
    if counts.old_count.is_zero() && counts.new_count.is_zero() {
        return Err(format!(
            "{path} has a hunk that declares no lines on either side ({shown}), so it states no \
             change; git does not emit one, and the empty edit it would yield reads as a change \
             that was checked and found clean"
        ));
    }
    if counts.old_count.is_zero() && !target.new_file {
        return Err(format!(
            "{path} has a hunk with an empty pre-image ({shown}) into a file that already exists, \
             so the hunk anchors to nothing; a zero-context hunk is the usual cause, so \
             regenerate the diff with context lines (drop -U0)"
        ));
    }
    if counts.old_start < *floor {
        return Err(format!(
            "{path} has hunks whose pre-image ranges overlap or run backwards: {shown} starts at \
             line {}, which the hunk before it already covered; a file's hunks describe one \
             state of it, so no edit list holds both",
            counts.old_start
        ));
    }
    Ok(counts)
}

// The header `@@ -(\d+)(,(\d+))? \+(\d+)(,(\d+))? @@`, with any Unicode decimal digit; the new
// side's start is read and not kept.
fn parse_header(header: &str) -> Option<Counts> {
    let rest = header.strip_prefix("@@ -")?;
    let (old_start, rest) = leading(rest)?;
    let (old_count, rest) = count(rest)?;
    let (_, rest) = leading(rest.strip_prefix(" +")?)?;
    let (new_count, rest) = count(rest)?;
    rest.starts_with(" @@").then_some(Counts {
        old_start,
        old_count,
        new_count,
    })
}

// An optional `,count`: a comma with no digits after it fails the whole header.
fn count(rest: &str) -> Option<(Natural, &str)> {
    match rest.strip_prefix(',') {
        Some(digits) => leading(digits),
        None => Some((Natural::one(), rest)),
    }
}

// The line after the hunk at `at`, or `None` when its header or body does not read.
pub(super) fn end_of_hunk(lines: &[&str], at: usize) -> Option<usize> {
    let header = lines[at];
    let counts = parse_header(header)?;
    let body = hunk_body(
        lines,
        at + 1,
        [&counts.old_count, &counts.new_count],
        "",
        header,
    );
    body.ok().map(|body| body.next)
}

fn count_mismatch(path: &str, header: &str) -> String {
    format!(
        "{path}: hunk {} declares line counts its body does not match (truncated or hand-edited \
         diff?)",
        text::strip(header)
    )
}

// A hunk body line that no header declared. A blank line and `--` follow a correct hunk in real
// output, so both read as the hunk's end.
fn is_stray_body(line: &str) -> bool {
    if line.is_empty() || trim_end(line) == TRAILER {
        return false;
    }
    if line.starts_with("@@") || opens_section(line) {
        return false;
    }
    line.starts_with([' ', '-', '+'])
}

// Whether the section still holds hunk content that the hunk loop did not read: a hunk header
// before the section's end, or body after the blank and `--` lines.
fn unread_content(lines: &[&str], at: usize, end: usize) -> bool {
    let rest = lines.get(at..end.min(lines.len())).unwrap_or_default();
    if rest.iter().any(|line| parse_header(line).is_some()) {
        return true;
    }
    rest.iter()
        .find(|line| !line.is_empty() && trim_end(line) != TRAILER)
        .is_some_and(|line| is_stray_body(line))
}

// One hunk's two sides, and the first line after it.
struct Body {
    search: String,
    replace: String,
    next: usize,
}

// Consumes exactly the lines that the header declares, so a body line that reads like the next
// file's header is content.
fn hunk_body(
    lines: &[&str],
    at: usize,
    counts: [&Natural; 2],
    path: &str,
    header: &str,
) -> Result<Body, String> {
    let mut tally = Tally::new(counts);
    let mut next = at;
    while tally.open() {
        let Some(&line) = lines.get(next) else {
            return Err(tally.truncated(path, header));
        };
        next += 1;
        // The no-newline marker carries a fact that an edit has nowhere to hold.
        if line.starts_with(NO_NEWLINE) {
            continue;
        }
        let (marker, content) = marked(line);
        match tally.fits(marker) {
            Some(true) => tally.take(marker, content),
            Some(false) => return Err(count_mismatch(path, header)),
            None => {
                return Err(format!(
                    "{path}: hunk {} has a body line that is not ' ', '-', '+' or '\\': {}",
                    text::strip(header),
                    quoted(line)
                ));
            }
        }
    }
    let markers = lines[next..]
        .iter()
        .take_while(|line| line.starts_with(NO_NEWLINE))
        .count();
    Ok(Body {
        search: tally.sides[OLD].join("\n"),
        replace: tally.sides[NEW].join("\n"),
        next: next + markers,
    })
}

// A bare empty line is context for an empty line: editors strip the single space.
fn marked(line: &str) -> (char, &str) {
    line.chars()
        .next()
        .map_or((' ', ""), |marker| (marker, &line[marker.len_utf8()..]))
}

// The lines one hunk body has taken on each side, against the counts its header declared.
struct Tally<'a> {
    counts: [&'a Natural; 2],
    // Each count as a usize, or `None` for a count above every usize, which never runs out.
    limits: [Option<usize>; 2],
    used: [usize; 2],
    sides: [Vec<&'a str>; 2],
}

impl<'a> Tally<'a> {
    fn new(counts: [&'a Natural; 2]) -> Tally<'a> {
        Tally {
            counts,
            limits: counts.map(Natural::to_usize),
            used: [0, 0],
            sides: [Vec::new(), Vec::new()],
        }
    }

    fn left(&self, side: usize) -> bool {
        self.limits[side].is_none_or(|limit| limit > self.used[side])
    }

    fn open(&self) -> bool {
        self.left(OLD) || self.left(NEW)
    }

    // Whether a line with `marker` fits the counts, or `None` for a marker no body line has.
    fn fits(&self, marker: char) -> Option<bool> {
        match marker {
            ' ' => Some(self.left(OLD) && self.left(NEW)),
            '-' => Some(self.left(OLD)),
            '+' => Some(self.left(NEW)),
            _ => None,
        }
    }

    fn take(&mut self, marker: char, content: &'a str) {
        let sides: &[usize] = match marker {
            ' ' => &[OLD, NEW],
            '-' => &[OLD],
            _ => &[NEW],
        };
        for &side in sides {
            self.used[side] += 1;
            self.sides[side].push(content);
        }
    }

    fn truncated(&self, path: &str, header: &str) -> String {
        format!(
            "{path}: hunk {} ends after part of its body, with {} pre-image and {} post-image \
             line(s) still to come (truncated diff?)",
            text::strip(header),
            self.counts[OLD].minus(self.used[OLD]),
            self.counts[NEW].minus(self.used[NEW]),
        )
    }
}
