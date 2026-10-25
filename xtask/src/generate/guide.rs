//! The agent guide: its addressable sections, and the Rust module that holds the text with a byte
//! table of the sections.
//!
//! A heading counts only outside a fenced block. A fence closes on a run of its own character at
//! least as long, with nothing but whitespace after it. Levels 2 and 3 are sections, and a section
//! runs to the next heading of the same or a shallower level.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::ops::Range;

use super::literal::literal;

/// The guide file, from the repository root.
pub const SOURCE: &str = "crates/app/data/agent-guide.md";
/// The directory of the generated module, from the repository root.
pub const OUTPUT: &str = "crates/app/src/generated";
/// The generated module's file name.
pub const FILE: &str = "guide.rs";

const SECTION_LEVELS: [usize; 2] = [2, 3];
const MAX_INDENT: usize = 3;
const MAX_LEVEL: usize = 6;
const MIN_FENCE_RUN: usize = 3;

/// One addressable section: its name and the byte range of its text in the guide.
#[derive(Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub text: Range<usize>,
}

/// Returns the guide's sections in document order, or each problem with the text.
pub fn sections(text: &str) -> Result<Vec<Section>, Vec<String>> {
    if let Some(found) = text.chars().find(|&c| breaks_the_line_model(c)) {
        let code = u32::from(found);
        return Err(vec![format!(
            "{SOURCE}: U+{code:04X} is refused: the guide holds no control character but tab and \
             newline, so each section is one byte range of the text"
        )]);
    }
    let lines = lines(text);
    let headings = headings(text, &lines);
    let found: Vec<Section> = headings
        .iter()
        .enumerate()
        .filter(|(_, heading)| SECTION_LEVELS.contains(&heading.level))
        .map(|(position, heading)| section(text, &lines, &headings[position + 1..], heading))
        .collect();
    let duplicates = duplicates(&found);
    if duplicates.is_empty() {
        Ok(found)
    } else {
        Err(duplicates
            .iter()
            .map(|name| {
                format!(
                    "{SOURCE}: two headings derive the section name {name:?}: one would shadow \
                     the other, so rename one heading"
                )
            })
            .collect())
    }
}

/// Returns the Rust module that holds `text` and its section table.
pub fn module(text: &str) -> Result<String, Vec<String>> {
    let sections = sections(text)?;
    let mut out = format!(
        "//! The agent guide, and each section's name with the byte range of its text.\n\
         //! `cargo xtask generate` writes this file from {SOURCE}.\n\n\
         pub(crate) const GUIDE: &str = {};\n\n\
         pub(crate) const SECTIONS: [(&str, usize, usize); {}] = [\n",
        literal(text),
        sections.len()
    );
    for section in &sections {
        let Range { start, end } = section.text;
        let (start, end) = (grouped(start), grouped(end));
        writeln!(out, "({}, {start}, {end}),", literal(&section.name))
            .expect("writing to a String cannot fail");
    }
    out.push_str("];\n");
    Ok(out)
}

// The digits of `offset` with `_` between groups of three from the right: clippy refuses a long
// literal with no separators.
fn grouped(offset: usize) -> String {
    let digits = offset.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.char_indices() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push('_');
        }
        out.push(digit);
    }
    out
}

// Unicode line splitting also ends a line at these, so a guide's lines would not be its newline
// ranges. Outside them, char::is_whitespace is the whitespace a heading allows.
fn breaks_the_line_model(c: char) -> bool {
    (c.is_control() && c != '\n' && c != '\t') || matches!(c, '\u{2028}' | '\u{2029}')
}

// The byte range of each line, without its newline. A final newline ends a line and starts none.
fn lines(text: &str) -> Vec<Range<usize>> {
    let mut start = 0;
    text.split_inclusive('\n')
        .map(|line| {
            let range = start..start + line.trim_end_matches('\n').len();
            start += line.len();
            range
        })
        .collect()
}

struct Heading<'t> {
    line: usize,
    level: usize,
    title: &'t str,
}

fn headings<'t>(text: &'t str, lines: &[Range<usize>]) -> Vec<Heading<'t>> {
    let mut found = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (index, range) in lines.iter().enumerate() {
        let line = &text[range.clone()];
        if let Some(opener) = fence_line(line) {
            fence = next_fence(fence, &opener);
            continue;
        }
        if fence.is_some() {
            continue;
        }
        if let Some((level, title)) = heading(line) {
            found.push(Heading {
                line: index,
                level,
                title,
            });
        }
    }
    found
}

struct Fence<'t> {
    mark: char,
    run: usize,
    info: &'t str,
}

// Up to three spaces, then a run of three or more backticks or tildes, then the info text.
fn fence_line(line: &str) -> Option<Fence<'_>> {
    let rest = indented(line)?;
    let mark = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let info = rest.trim_start_matches(mark);
    let run = rest.len() - info.len();
    (run >= MIN_FENCE_RUN).then_some(Fence { mark, run, info })
}

fn next_fence(open: Option<(char, usize)>, line: &Fence<'_>) -> Option<(char, usize)> {
    match open {
        None => Some((line.mark, line.run)),
        Some((mark, run))
            if line.mark == mark && line.run >= run && line.info.trim().is_empty() =>
        {
            None
        }
        still_open => still_open,
    }
}

// Up to three spaces, one to six '#', whitespace, then the title without its closing hashes.
fn heading(line: &str) -> Option<(usize, &str)> {
    let rest = indented(line)?;
    let after = rest.trim_start_matches('#');
    let level = rest.len() - after.len();
    let title = after.trim_start_matches(char::is_whitespace);
    let spaced = title.len() < after.len();
    ((1..=MAX_LEVEL).contains(&level) && spaced).then(|| (level, title_text(title)))
}

fn indented(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(' ');
    (line.len() - rest.len() <= MAX_INDENT).then_some(rest)
}

// The shortest prefix whose remainder is whitespace, then hashes, then whitespace.
fn title_text(title: &str) -> &str {
    let mut ends = title.char_indices().map(|(at, _)| at).chain([title.len()]);
    let end = ends
        .find(|&at| closing(&title[at..]))
        .unwrap_or(title.len());
    &title[..end]
}

fn closing(tail: &str) -> bool {
    tail.trim_start_matches(char::is_whitespace)
        .trim_start_matches('#')
        .chars()
        .all(char::is_whitespace)
}

// The section ends before the next heading of the same or a shallower level, with its trailing
// newlines dropped.
fn section(
    text: &str,
    lines: &[Range<usize>],
    following: &[Heading<'_>],
    heading: &Heading<'_>,
) -> Section {
    let end_line = following
        .iter()
        .find(|next| next.level <= heading.level)
        .map_or(lines.len(), |next| next.line);
    let start = lines[heading.line].start;
    let end = start
        + text[start..lines[end_line - 1].end]
            .trim_end_matches('\n')
            .len();
    Section {
        name: slug(heading.title),
        text: start..end,
    }
}

// The prose after the first " -- " goes, then each parenthesised part and each backtick, then
// every run of characters that are not ASCII letters or digits becomes one hyphen.
fn slug(title: &str) -> String {
    let head = title.split_once(" -- ").map_or(title, |(head, _)| head);
    let text = without_parenthesised(head).replace('`', "");
    let mut slug = String::new();
    let mut in_run = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            in_run = false;
        } else if !in_run {
            slug.push('-');
            in_run = true;
        }
    }
    slug.trim_matches('-').to_owned()
}

fn without_parenthesised(text: &str) -> String {
    let mut kept = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        let Some(length) = rest[open..].find(')') else {
            break;
        };
        kept.push_str(&rest[..open]);
        kept.push(' ');
        rest = &rest[open + length + 1..];
    }
    kept.push_str(rest);
    kept
}

fn duplicates(sections: &[Section]) -> BTreeSet<&str> {
    let mut seen = BTreeSet::new();
    sections
        .iter()
        .map(|section| section.name.as_str())
        .filter(|name| !seen.insert(*name))
        .collect()
}

#[cfg(test)]
mod tests;
