//! The matching blocks of two sequences of lines, as the longest-match sequence matcher finds
//! them with automatic junk on.

use std::collections::HashMap;

// One matching block: the start in `a`, the start in `b`, and the length.
type Block = (usize, usize, usize);

/// Returns the matching blocks of `a` and `b` as (start in `a`, start in `b`, length), in
/// order, adjacent blocks merged, and then `(a.len(), b.len(), 0)`. When `b` holds 200 or more
/// lines, a line that occurs more than `b.len() / 100 + 1` times in `b` starts no match, and a
/// match can still extend across it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::cases;

    #[test]
    fn matching_blocks_cases() {
        let all: Vec<(Vec<String>, Vec<String>, Vec<Block>)> = cases("difflib", "matching_blocks");
        for (a, b, expected) in all {
            let a: Vec<&str> = a.iter().map(String::as_str).collect();
            let b: Vec<&str> = b.iter().map(String::as_str).collect();
            assert_eq!(matching_blocks(&a, &b), expected, "a: {a:?}\nb: {b:?}");
        }
    }
}
