use text::is_space;

const ELLIPSIS: &str = "...";
// A signature is an index row on one line, not a rendering of the declaration.
const SIGNATURE_MAX_CHARS: usize = 80;
const QUOTES: [&str; 5] = ["\"\"\"", "'''", "\"", "'", "`"];

/// Return the name that a base-class expression looks up: `Generic[T]` is `Generic`, `enum.Enum`
/// is `Enum`, and a keyword argument such as `metaclass=ABCMeta` is empty.
#[must_use]
pub fn base_name(text: &str) -> &str {
    let head = text::strip(text.split('[').next().unwrap_or_default());
    if head.is_empty() || head.contains('=') {
        return "";
    }
    text::strip(head.rsplit('.').next().unwrap_or_default())
}

// Cap `text` at `limit` code points, the last three of them `...` when it is cut.
pub(crate) fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text
        .chars()
        .take(limit.saturating_sub(ELLIPSIS.len()))
        .collect();
    kept + ELLIPSIS
}

// Collapse each whitespace run to one space, strip the ends, and cap the result. The join stops
// past the cap: the cut depends only on that and on the first code points.
pub(crate) fn normalise_signature(signature: &str) -> String {
    let mut joined = String::new();
    let mut chars = 0;
    for word in text::split_whitespace(signature) {
        if chars > SIGNATURE_MAX_CHARS {
            break;
        }
        if !joined.is_empty() {
            joined.push(' ');
            chars += 1;
        }
        joined.push_str(word);
        chars += word.chars().count();
    }
    truncate(&joined, SIGNATURE_MAX_CHARS)
}

// Strip the whitespace, then one pair of matching quotes of the first kind that wraps the text.
pub(crate) fn strip_quotes(text: &str) -> &str {
    let text = text::strip(text);
    QUOTES
        .iter()
        .find(|quote| {
            text.chars().count() >= 2 * quote.len()
                && text.starts_with(**quote)
                && text.ends_with(**quote)
        })
        .map_or(text, |quote| &text[quote.len()..text.len() - quote.len()])
}

pub(crate) fn lstrip(text: &str) -> &str {
    text.trim_start_matches(is_space)
}

pub(crate) fn rstrip(text: &str) -> &str {
    text.trim_end_matches(is_space)
}

#[cfg(test)]
mod tests {
    use text::testing::cases;

    use super::{base_name, normalise_signature, strip_quotes, truncate};

    fn manifest() -> &'static str {
        env!("CARGO_MANIFEST_DIR")
    }

    #[test]
    fn each_base_name_is_the_reference_name() {
        let all: Vec<(String, String)> = cases(manifest(), "views", "base_name");
        assert!(all.len() >= 7);
        for (text, expected) in all {
            assert_eq!(base_name(&text), expected, "{text:?}");
        }
    }

    #[test]
    fn each_signature_and_cut_is_the_reference_text() {
        let signatures: Vec<(String, String)> = cases(manifest(), "views", "signatures");
        for (text, expected) in signatures {
            assert_eq!(normalise_signature(&text), expected, "{text:?}");
        }
        let cuts: Vec<(String, usize, String)> = cases(manifest(), "views", "truncate");
        for (text, limit, expected) in cuts {
            assert_eq!(truncate(&text, limit), expected, "{text:?} at {limit}");
        }
    }

    #[test]
    fn a_signature_near_the_cap_is_the_cut_of_the_whole_join() {
        let x = |count: usize| "x".repeat(count);
        let pinned = [
            (format!("{} ab", x(77)), format!("{} ab", x(77))),
            (format!("{}\nabc", x(77)), format!("{}...", x(77))),
            (format!("{}\n\n  y z", x(76)), format!("{} y z", x(76))),
            (format!("{}\t yy", x(78)), format!("{}...", x(77))),
            (format!("k = {}\r\n w2", x(73)), format!("k = {} w2", x(73))),
        ];
        for (text, expected) in pinned {
            assert_eq!(normalise_signature(&text), expected, "{text:?}");
        }
        for head in 74..=84 {
            for gap in [" ", "\n", "\r\n", " \n\t "] {
                let text = format!("{}{gap}yy{gap}z{gap}{}{gap}", x(head), "w".repeat(90));
                let whole: Vec<&str> = text::split_whitespace(&text).collect();
                let expected = truncate(&whole.join(" "), 80);
                assert_eq!(normalise_signature(&text), expected, "{text:?}");
            }
        }
    }

    #[test]
    fn one_pair_of_the_first_wrapping_quote_comes_off() {
        let cases = [
            (" \"a\" ", "a"),
            ("'''doc'''", "doc"),
            ("'''", "'"),
            ("\"\"", ""),
            ("\"", "\""),
            ("`cmd`", "cmd"),
            ("'a\"", "'a\""),
            ("\"\"\"a\"\"", "\"\"a\""),
        ];
        for (text, expected) in cases {
            assert_eq!(strip_quotes(text), expected, "{text:?}");
        }
    }
}
