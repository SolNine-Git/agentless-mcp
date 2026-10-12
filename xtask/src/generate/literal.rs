//! Rust string literals for generated code, with long text broken over lines.

// Long enough to keep few lines, short enough that each line stays within 100 characters when
// rustfmt places the literal in an array inside a call.
const WIDTH: usize = 64;
const CONTINUATION: &str = "\\\n        ";

/// Returns `text` as a Rust string literal that continues on a new line after a space.
pub fn literal(text: &str) -> String {
    literal_within(text, WIDTH)
}

/// Returns `text` as `literal` does, with at most `first` bytes on the first line unless its first
/// word is longer.
pub fn literal_within(text: &str, first: usize) -> String {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in words(text) {
        let width = if lines.is_empty() { first } else { WIDTH };
        if !line.is_empty() && line.len() + word.len() > width {
            lines.push(std::mem::take(&mut line));
        }
        line.push_str(&word);
    }
    lines.push(line);
    format!("\"{}\"", lines.join(CONTINUATION))
}

// Escaped text cut after each space whose next character is not whitespace: a continuation skips
// the whitespace that starts the next line.
fn words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        word.push_str(&escape(c));
        let next_is_visible = chars
            .peek()
            .is_some_and(|next| !escape(*next).starts_with(char::is_whitespace));
        if c == ' ' && next_is_visible {
            words.push(std::mem::take(&mut word));
        }
    }
    words.push(word);
    words
}

fn escape(c: char) -> String {
    match c {
        '"' => "\\\"".to_owned(),
        '\\' => "\\\\".to_owned(),
        '\n' => "\\n".to_owned(),
        '\r' => "\\r".to_owned(),
        '\t' => "\\t".to_owned(),
        // Outside ASCII a character can be invisible, and rustc refuses a raw U+202E in a literal.
        c if c.is_ascii_control() || !c.is_ascii() => format!("\\u{{{:x}}}", u32::from(c)),
        c => c.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{WIDTH, literal};

    fn rejoined(made: &str) -> String {
        made.split("\\\n").map(str::trim_start).collect()
    }

    #[test]
    fn quotes_backslashes_and_control_characters_are_escaped() {
        assert_eq!(
            literal("a\"b\\c\nd\re\tf\u{1}g\u{7f}"),
            r#""a\"b\\c\nd\re\tf\u{1}g\u{7f}""#
        );
    }

    #[test]
    fn every_character_outside_ascii_is_escaped() {
        assert_eq!(literal("{x} \u{e9}"), r#""{x} \u{e9}""#);
        assert_eq!(
            literal("a\u{202e}b\u{85}\u{a0}\u{1f600}"),
            r#""a\u{202e}b\u{85}\u{a0}\u{1f600}""#
        );
    }

    #[test]
    fn long_text_breaks_after_a_space_into_lines_of_at_most_the_width() {
        let text = "word ".repeat(40);
        let made = literal(text.trim_end());
        let lines: Vec<&str> = made.split("\\\n").collect();
        assert!(lines.len() > 1, "{made}");
        assert!(
            lines
                .iter()
                .all(|line| line.trim_start().len() <= WIDTH + 1)
        );
        assert_eq!(rejoined(&made), format!("\"{}\"", text.trim_end()));
    }

    #[test]
    fn a_break_never_comes_before_whitespace() {
        let long = "a".repeat(WIDTH);
        let text = format!("{long}  {long}\n\t{long} \u{a0}{long}");
        let made = literal(&text);
        for line in made.split("\\\n").skip(1) {
            let first = line.trim_start().chars().next();
            assert!(first.is_some_and(|c| !c.is_whitespace()), "{made}");
        }
        let escaped = format!("\"{long}  {long}\\n\\t{long} \\u{{a0}}{long}\"");
        assert_eq!(rejoined(&made), escaped);
    }
}
