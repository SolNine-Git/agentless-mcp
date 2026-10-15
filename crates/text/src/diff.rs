//! The matching blocks of two sequences of lines, as the longest-match sequence matcher finds
//! them with automatic junk on, and the unified diff built on them.

use std::collections::HashMap;
use std::mem;

// One matching block: the start in `a`, the start in `b`, and the length.
type Block = (usize, usize, usize);

// The equal lines a unified diff shows before and after each change.
const CONTEXT: usize = 3;

/// Returns the blocks of equal lines that `a` and `b` share, in order, and an end block of size 0.
#[must_use]
pub fn matching_blocks(a: &[&str], b: &[&str]) -> Vec<(usize, usize, usize)> {
    let matcher = Matcher {
        a,
        b,
        positions: positions(b),
    };
    let mut found = matcher.blocks();
    found.sort_unstable();
    let mut merged = merge_adjacent(&found);
    merged.push((a.len(), b.len(), 0));
    merged
}

/// Returns the unified diff lines from `a` to `b`, with empty file names and no line ends.
#[must_use]
pub fn unified_diff(a: &[&str], b: &[&str]) -> Vec<String> {
    let groups = grouped_opcodes(a, b, CONTEXT);
    if groups.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["--- ".to_owned(), "+++ ".to_owned()];
    for group in &groups {
        lines.extend(hunk(group, a, b));
    }
    lines
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tag {
    Replace,
    Delete,
    Insert,
    Equal,
}

// The lines `a_start..a_end` of `a` and `b_start..b_end` of `b`, and how the first become the
// second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Opcode {
    tag: Tag,
    a_start: usize,
    a_end: usize,
    b_start: usize,
    b_end: usize,
}

impl Opcode {
    // The first `count` lines of an equal run.
    fn head(self, count: usize) -> Self {
        Self {
            a_end: self.a_end.min(self.a_start + count),
            b_end: self.b_end.min(self.b_start + count),
            ..self
        }
    }

    // The last `count` lines of an equal run.
    fn tail(self, count: usize) -> Self {
        Self {
            a_start: self.a_start.max(self.a_end.saturating_sub(count)),
            b_start: self.b_start.max(self.b_end.saturating_sub(count)),
            ..self
        }
    }
}

#[derive(Clone, Copy)]
struct Span {
    a_low: usize,
    a_high: usize,
    b_low: usize,
    b_high: usize,
}

struct Matcher<'a> {
    a: &'a [&'a str],
    b: &'a [&'a str],
    // The positions of each line in `b`, without the popular lines.
    positions: HashMap<&'a str, Vec<usize>>,
}

fn positions<'a>(b: &[&'a str]) -> HashMap<&'a str, Vec<usize>> {
    let mut positions: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, &line) in b.iter().enumerate() {
        positions.entry(line).or_default().push(index);
    }
    if b.len() >= 200 {
        let most = b.len() / 100 + 1;
        positions.retain(|_, indices| indices.len() <= most);
    }
    positions
}

impl Matcher<'_> {
    // Splits each span at its longest match, the left part on top of the stack last.
    fn blocks(&self) -> Vec<Block> {
        let whole = Span {
            a_low: 0,
            a_high: self.a.len(),
            b_low: 0,
            b_high: self.b.len(),
        };
        let mut pending = vec![whole];
        let mut found = Vec::new();
        while let Some(span) = pending.pop() {
            let (i, j, size) = self.longest_match(span);
            if size == 0 {
                continue;
            }
            found.push((i, j, size));
            if span.a_low < i && span.b_low < j {
                pending.push(Span {
                    a_high: i,
                    b_high: j,
                    ..span
                });
            }
            if i + size < span.a_high && j + size < span.b_high {
                pending.push(Span {
                    a_low: i + size,
                    b_low: j + size,
                    ..span
                });
            }
        }
        found
    }

    fn longest_match(&self, span: Span) -> Block {
        let (mut i, mut j, mut size) = self.longest_indexed_match(span);
        while i > span.a_low && j > span.b_low && self.a[i - 1] == self.b[j - 1] {
            (i, j, size) = (i - 1, j - 1, size + 1);
        }
        while i + size < span.a_high
            && j + size < span.b_high
            && self.a[i + size] == self.b[j + size]
        {
            size += 1;
        }
        (i, j, size)
    }

    // The longest run of equal lines whose lines are not popular, the earliest of equal runs.
    fn longest_indexed_match(&self, span: Span) -> Block {
        let mut best = (span.a_low, span.b_low, 0);
        let mut run_ends = HashMap::new();
        for i in span.a_low..span.a_high {
            let (ends, longest) = self.runs_ending_at(i, span, &run_ends);
            if longest.2 > best.2 {
                best = longest;
            }
            run_ends = ends;
        }
        best
    }

    // The length of each run that ends at line `i` of `a`, keyed by its end in `b`, and the
    // first of the longest of them.
    fn runs_ending_at(
        &self,
        i: usize,
        span: Span,
        previous: &HashMap<usize, usize>,
    ) -> (HashMap<usize, usize>, Block) {
        let mut ends = HashMap::new();
        let mut longest = (i, span.b_low, 0);
        let indices = self.positions.get(self.a[i]).map_or(&[][..], Vec::as_slice);
        let in_span = indices
            .iter()
            .skip_while(|&&j| j < span.b_low)
            .take_while(|&&j| j < span.b_high);
        for &j in in_span {
            let before = j.checked_sub(1).and_then(|end| previous.get(&end));
            let size = before.map_or(1, |run| run + 1);
            ends.insert(j, size);
            if size > longest.2 {
                longest = (i + 1 - size, j + 1 - size, size);
            }
        }
        (ends, longest)
    }
}

fn merge_adjacent(found: &[Block]) -> Vec<Block> {
    let mut merged = Vec::new();
    let mut current = (0, 0, 0);
    for &(i, j, size) in found {
        if current.0 + current.2 == i && current.1 + current.2 == j {
            current.2 += size;
            continue;
        }
        if current.2 > 0 {
            merged.push(current);
        }
        current = (i, j, size);
    }
    if current.2 > 0 {
        merged.push(current);
    }
    merged
}

// The groups of changes, each with at most `context` equal lines around it. An equal run longer
// than twice `context` ends one group and starts the next.
fn grouped_opcodes(a: &[&str], b: &[&str], context: usize) -> Vec<Vec<Opcode>> {
    let mut groups = Vec::new();
    let mut group = Vec::new();
    for mut code in trimmed_ends(opcodes(a, b), context) {
        if code.tag == Tag::Equal && code.a_end - code.a_start > 2 * context {
            group.push(code.head(context));
            groups.push(mem::take(&mut group));
            code = code.tail(context);
        }
        group.push(code);
    }
    let only_context = group.len() == 1 && group[0].tag == Tag::Equal;
    if !group.is_empty() && !only_context {
        groups.push(group);
    }
    groups
}

// The opcodes with at most `context` equal lines before the first change and after the last.
fn trimmed_ends(mut codes: Vec<Opcode>, context: usize) -> Vec<Opcode> {
    if let Some(first) = codes.first_mut().filter(|code| code.tag == Tag::Equal) {
        *first = first.tail(context);
    }
    if let Some(last) = codes.last_mut().filter(|code| code.tag == Tag::Equal) {
        *last = last.head(context);
    }
    codes
}

fn opcodes(a: &[&str], b: &[&str]) -> Vec<Opcode> {
    let (mut i, mut j) = (0, 0);
    let mut codes = Vec::new();
    for (a_at, b_at, size) in matching_blocks(a, b) {
        if let Some(tag) = change_tag(i < a_at, j < b_at) {
            codes.push(Opcode {
                tag,
                a_start: i,
                a_end: a_at,
                b_start: j,
                b_end: b_at,
            });
        }
        (i, j) = (a_at + size, b_at + size);
        if size > 0 {
            codes.push(Opcode {
                tag: Tag::Equal,
                a_start: a_at,
                a_end: i,
                b_start: b_at,
                b_end: j,
            });
        }
    }
    codes
}

fn change_tag(removes: bool, adds: bool) -> Option<Tag> {
    match (removes, adds) {
        (true, true) => Some(Tag::Replace),
        (true, false) => Some(Tag::Delete),
        (false, true) => Some(Tag::Insert),
        (false, false) => None,
    }
}

// A delete adds no lines and an insert removes none, so each change writes its removed lines and
// then its added lines.
fn hunk(group: &[Opcode], a: &[&str], b: &[&str]) -> Vec<String> {
    let (first, last) = (group[0], group[group.len() - 1]);
    let old = hunk_range(first.a_start, last.a_end);
    let new = hunk_range(first.b_start, last.b_end);
    let mut lines = vec![format!("@@ -{old} +{new} @@")];
    for code in group {
        let old_lines = &a[code.a_start..code.a_end];
        if code.tag == Tag::Equal {
            lines.extend(marked(' ', old_lines));
            continue;
        }
        lines.extend(marked('-', old_lines));
        lines.extend(marked('+', &b[code.b_start..code.b_end]));
    }
    lines
}

fn marked<'a>(mark: char, lines: &'a [&str]) -> impl Iterator<Item = String> + 'a {
    lines.iter().map(move |line| format!("{mark}{line}"))
}

// A hunk range: the first line and the count, the count left out when it is 1. An empty range
// names the line before it.
fn hunk_range(start: usize, end: usize) -> String {
    match end - start {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        count => format!("{},{count}", start + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::cases;
    use serde::de::DeserializeOwned;
    use std::fmt::Debug;

    // An opcode as the fixtures write it: the tag name and the four bounds.
    type Named = (String, usize, usize, usize, usize);
    // A grouped-opcodes case: the two sides, the context size, and the groups.
    type GroupCase = (Vec<String>, Vec<String>, usize, Vec<Vec<Named>>);

    #[test]
    fn matching_blocks_cases() {
        check_line_pairs("matching_blocks", matching_blocks);
    }

    #[test]
    fn opcodes_cases() {
        check_line_pairs("opcodes", |a, b| {
            opcodes(a, b).into_iter().map(named).collect::<Vec<_>>()
        });
    }

    #[test]
    fn grouped_opcodes_cases() {
        let all: Vec<GroupCase> = cases(env!("CARGO_MANIFEST_DIR"), "diff", "grouped_opcodes");
        for (a, b, context, expected) in all {
            let (a, b) = (borrowed(&a), borrowed(&b));
            let found: Vec<Vec<Named>> = grouped_opcodes(&a, &b, context)
                .into_iter()
                .map(|group| group.into_iter().map(named).collect())
                .collect();
            assert_eq!(found, expected, "context {context}\na: {a:?}\nb: {b:?}");
        }
    }

    #[test]
    fn unified_diff_cases() {
        check_line_pairs("unified_diff", unified_diff);
    }

    // Checks `function` on each (a, b, expected) case of the section `section`.
    fn check_line_pairs<T>(section: &str, function: impl Fn(&[&str], &[&str]) -> T)
    where
        T: DeserializeOwned + PartialEq + Debug,
    {
        let all: Vec<(Vec<String>, Vec<String>, T)> =
            cases(env!("CARGO_MANIFEST_DIR"), "diff", section);
        for (a, b, expected) in all {
            let (a, b) = (borrowed(&a), borrowed(&b));
            assert_eq!(function(&a, &b), expected, "{section}\na: {a:?}\nb: {b:?}");
        }
    }

    fn borrowed(lines: &[String]) -> Vec<&str> {
        lines.iter().map(String::as_str).collect()
    }

    fn named(code: Opcode) -> Named {
        let tag = match code.tag {
            Tag::Replace => "replace",
            Tag::Delete => "delete",
            Tag::Insert => "insert",
            Tag::Equal => "equal",
        };
        (
            tag.to_owned(),
            code.a_start,
            code.a_end,
            code.b_start,
            code.b_end,
        )
    }
}
