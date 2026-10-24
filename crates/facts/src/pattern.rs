//! Hand-written matchers for the few patterns the extraction reads, with the word, digit and
//! space classes of regular expressions on text.

use text::{is_decimal, is_space, is_word};

use crate::values::RationaleKind;

const MARKERS: [(&str, RationaleKind); 4] = [
    ("NOTE", RationaleKind::Note),
    ("WHY", RationaleKind::Why),
    ("HACK", RationaleKind::Hack),
    ("TODO", RationaleKind::Todo),
];
const CITATIONS: [&str; 2] = ["adr", "rfc"];

// An upper-case name: an ASCII capital, then capitals, digits and underscores, to the end.
pub(crate) fn upper_case(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|first| first.is_ascii_uppercase())
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

// The same name where the end may also sit before one final newline.
pub(crate) fn upper_case_line(name: &str) -> bool {
    upper_case(name.strip_suffix('\n').unwrap_or(name))
}

// The first marker that opens a word, then optional spaces, a colon and optional spaces: its
// kind and the rest of the line.
pub(crate) fn marker(line: &str) -> Option<(RationaleKind, &str)> {
    line.char_indices().find_map(|(at, _)| marker_at(line, at))
}

// Each citation: `ADR` or `RFC` in any case, opening a word, then dashes, spaces, `#` or `:`, then
// digits that end a word; the matches do not overlap.
pub(crate) fn citations(line: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut at = 0;
    while at < line.len() {
        match citation_at(line, at) {
            Some(end) => {
                found.push(&line[at..end]);
                at = end;
            }
            None => at += line[at..].chars().next().map_or(1, char::len_utf8),
        }
    }
    found
}

fn marker_at(line: &str, at: usize) -> Option<(RationaleKind, &str)> {
    if !opens_word(line, at) {
        return None;
    }
    let rest = &line[at..];
    let (word, kind) = MARKERS.iter().find(|(word, _)| rest.starts_with(word))?;
    let rest = rest[word.len()..].trim_start_matches(is_space);
    let rest = rest.strip_prefix(':')?;
    Some((*kind, rest.trim_start_matches(is_space)))
}

fn citation_at(line: &str, at: usize) -> Option<usize> {
    if !opens_word(line, at) {
        return None;
    }
    let head = line.get(at..at + 3)?;
    if !CITATIONS.iter().any(|word| head.eq_ignore_ascii_case(word)) {
        return None;
    }
    let rest = line[at + 3..].trim_start_matches(|c| matches!(c, '-' | '#' | ':') || is_space(c));
    let digits = rest.len() - rest.trim_start_matches(is_decimal).len();
    let end = line.len() - rest.len() + digits;
    (digits > 0 && !line[end..].chars().next().is_some_and(is_word)).then_some(end)
}

// A word boundary before a word character: no word character sits just before it.
fn opens_word(line: &str, at: usize) -> bool {
    !line[..at].chars().next_back().is_some_and(is_word)
}

#[cfg(test)]
mod tests {
    use text::testing::cases;

    use super::{citations, marker, upper_case, upper_case_line};
    use crate::values::RationaleKind;

    #[test]
    fn each_upper_case_test_is_the_reference_outcome() {
        let all: Vec<(String, bool, bool)> =
            cases(env!("CARGO_MANIFEST_DIR"), "views", "upper_case");
        assert!(all.len() >= 10);
        for (name, matched, full) in all {
            assert_eq!(upper_case_line(&name), matched, "{name:?}");
            assert_eq!(upper_case(&name), full, "{name:?}");
        }
    }

    #[test]
    fn a_marker_opens_a_word_and_needs_a_colon() {
        assert_eq!(marker("NOTE: a"), Some((RationaleKind::Note, "a")));
        assert_eq!(marker("x WHY  :\t b "), Some((RationaleKind::Why, "b ")));
        assert_eq!(marker("\u{e9}NOTE: a"), None);
        assert_eq!(marker("_TODO: a TODO: b"), Some((RationaleKind::Todo, "b")));
        assert_eq!(marker("NOTE a"), None);
        assert_eq!(marker("HACK:"), Some((RationaleKind::Hack, "")));
    }

    #[test]
    fn a_citation_opens_and_ends_a_word() {
        assert_eq!(
            citations("ADR-12 and adr 7 and RFC#9 and rfc:: 10"),
            ["ADR-12", "adr 7", "RFC#9", "rfc:: 10"]
        );
        assert!(citations("XADR1 and ADR12x and ADR_3").is_empty());
        assert_eq!(citations("RFC \u{663}\u{664} x"), ["RFC \u{663}\u{664}"]);
        assert!(citations("ADR-\u{663}x").is_empty());
    }
}
