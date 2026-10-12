//! Line filling of ASCII help text: chunks split at spaces and after word hyphens, lines filled
//! greedily, and a word longer than a line broken after its last hyphen that fits.

use std::ops::Range;

pub(crate) fn collapse(text: &str) -> String {
    let words: Vec<&str> = text
        .split(is_space_char)
        .filter(|w| !w.is_empty())
        .collect();
    words.join(" ")
}

pub(crate) fn wrap(text: &str, width: usize) -> Vec<&str> {
    let mut chunks = split(text.as_bytes());
    chunks.reverse();
    let mut lines = Vec::new();
    while let Some(top) = chunks.last() {
        if !lines.is_empty() && is_blank(text, top) {
            chunks.pop();
        }
        if let Some(line) = fill_line(text, &mut chunks, width) {
            lines.push(line);
        }
    }
    lines
}

// Takes the chunks of one line from the top of the reversed `chunks`.
fn fill_line<'t>(text: &'t str, chunks: &mut Vec<Range<usize>>, width: usize) -> Option<&'t str> {
    let mut line: Vec<Range<usize>> = Vec::new();
    let mut length = 0;
    while let Some(next) = chunks.last() {
        if length + next.len() > width {
            break;
        }
        length += next.len();
        line.extend(chunks.pop());
    }
    if let Some(top) = chunks.last_mut().filter(|next| next.len() > width) {
        break_long_word(
            text.as_bytes(),
            top,
            &mut line,
            width.saturating_sub(length),
        );
    }
    if line.last().is_some_and(|last| is_blank(text, last)) {
        line.pop();
    }
    let (first, last) = (line.first()?, line.last()?);
    Some(&text[first.start..last.end])
}

// Moves the head of `top` that fits in `space` into `line`: up to its last hyphen inside that
// space when a non-hyphen precedes the hyphen, else exactly `space` bytes.
fn break_long_word(
    text: &[u8],
    top: &mut Range<usize>,
    line: &mut Vec<Range<usize>>,
    space: usize,
) {
    if space == 0 {
        return;
    }
    let word = &text[top.clone()];
    let mut end = space;
    if let Some(hyphen) = word[..space].iter().rposition(|&b| b == b'-')
        && hyphen > 0
        && word[..hyphen].iter().any(|&b| b != b'-')
    {
        end = hyphen + 1;
    }
    line.push(top.start..top.start + end);
    top.start += end;
}

fn is_blank(text: &str, chunk: &Range<usize>) -> bool {
    text[chunk.clone()].bytes().all(is_space)
}

// Splits `text` into chunks: runs of spaces, dash runs between words, and words that end at a
// space, at the end, before a dash run, or after a hyphen between letters.
fn split(text: &[u8]) -> Vec<Range<usize>> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = if is_space(text[start]) {
            start + text[start..].iter().take_while(|&&b| is_space(b)).count()
        } else if let Some(end) = dash_run(text, start) {
            end
        } else {
            word_end(text, start)
        };
        chunks.push(start..end);
        start = end;
    }
    chunks
}

// Two or more dashes after word punctuation and before a word character.
fn dash_run(text: &[u8], start: usize) -> Option<usize> {
    let before = text[start.checked_sub(1)?];
    (is_punct(before) && dashes_then_word(text, start)).then(|| start + dashes(text, start))
}

// The end of the shortest non-space run from `start` that a word boundary closes.
fn word_end(text: &[u8], start: usize) -> usize {
    for end in start + 1..text.len() {
        if text[end] == b'-' && letters_before(text, end) && letters_after(text, end) {
            return end + 1;
        }
        if is_space(text[end]) || (is_punct(text[end - 1]) && dashes_then_word(text, end)) {
            return end;
        }
    }
    text.len()
}

// Two letters, or a letter, a hyphen and a letter, end just before the hyphen at `at`.
fn letters_before(text: &[u8], at: usize) -> bool {
    let letter = |back: usize| at.checked_sub(back).is_some_and(|o| is_letter(text[o]));
    let hyphen = at.checked_sub(2).is_some_and(|o| text[o] == b'-');
    (letter(2) && letter(1)) || (letter(3) && hyphen && letter(1))
}

// A letter, an optional hyphen and a letter start just after the hyphen at `at`.
fn letters_after(text: &[u8], at: usize) -> bool {
    let letter = |offset: usize| text.get(offset).is_some_and(|&b| is_letter(b));
    let hyphen = text.get(at + 2) == Some(&b'-');
    letter(at + 1) && (letter(at + 2) || (hyphen && letter(at + 3)))
}

fn dashes(text: &[u8], start: usize) -> usize {
    text[start..].iter().take_while(|&&b| b == b'-').count()
}

fn dashes_then_word(text: &[u8], start: usize) -> bool {
    let count = dashes(text, start);
    count >= 2 && text.get(start + count).is_some_and(|&b| is_word(b))
}

fn is_space(b: u8) -> bool {
    matches!(b, b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' ')
}

fn is_space_char(c: char) -> bool {
    u8::try_from(c).is_ok_and(is_space)
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_letter(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_punct(b: u8) -> bool {
    is_word(b) || b"!\"'&.,?".contains(&b)
}
