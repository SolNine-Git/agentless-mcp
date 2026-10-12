//! Whitespace and line-break rules that the tokenizer and the checks share.

/// Returns whether `c` is whitespace; unlike `char::is_whitespace`, U+001C to U+001F count.
#[must_use]
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\t'..='\r'
            | '\u{1c}'..='\u{1f}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// Splits `s` at line breaks, with `\r\n` as one break; a final break adds no empty line.
/// `keepends` keeps each break at the end of its line.
#[must_use]
pub fn splitlines(s: &str, keepends: bool) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((break_start, c)) = chars.next() {
        if !matches!(
            c,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        ) {
            continue;
        }
        let break_end = if c == '\r' && chars.next_if(|&(_, next)| next == '\n').is_some() {
            break_start + 2
        } else {
            break_start + c.len_utf8()
        };
        lines.push(&s[start..if keepends { break_end } else { break_start }]);
        start = break_end;
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// Yields the non-empty fields of `s` between runs of [`is_space`] characters.
pub fn split_whitespace(s: &str) -> impl Iterator<Item = &str> {
    s.split(is_space).filter(|part| !part.is_empty())
}

/// Removes [`is_space`] characters from both ends of `s`.
#[must_use]
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{cases, check_text_cases};
    use std::collections::BTreeSet;

    #[test]
    fn is_space_cases() {
        let spaces: BTreeSet<u32> = cases("text", "is_space");
        for c in '\0'..=char::MAX {
            let expected = spaces.contains(&u32::from(c));
            assert_eq!(is_space(c), expected, "mismatch for {c:?}");
        }
    }

    #[test]
    fn splitlines_cases() {
        let all: Vec<(String, bool, Vec<String>)> = cases("text", "splitlines");
        for (input, keepends, expected) in all {
            assert_eq!(splitlines(&input, keepends), expected, "input: {input:?}");
        }
    }

    #[test]
    fn split_whitespace_cases() {
        let all: Vec<(String, Vec<String>)> = cases("text", "split_whitespace");
        for (input, expected) in all {
            let fields: Vec<&str> = split_whitespace(&input).collect();
            assert_eq!(fields, expected, "input: {input:?}");
        }
    }

    #[test]
    fn strip_cases() {
        check_text_cases("strip", |input| strip(input).to_owned());
    }
}
