//! The decimal integer text rule: whitespace around an optional sign and grouped ASCII digits.

use crate::{is_ascii_space, is_space, strip_bytes};

// The most digits, leading zeros included, that an integer conversion of a text accepts.
const MAX_CONVERTIBLE_DIGITS: usize = 4300;

/// A decimal integer text without the whitespace around it: a sign, and ASCII digits with single
/// underscores between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntText<'a> {
    /// Whether the text starts with `-`.
    pub negative: bool,
    grouped: &'a [u8],
}

impl<'a> IntText<'a> {
    /// Yields the value of each digit, the most significant first, without the underscores.
    pub fn digits(self) -> impl Iterator<Item = u8> + 'a {
        self.grouped
            .iter()
            .filter(|&&byte| byte != b'_')
            .map(|&byte| byte - b'0')
    }
}

/// Reads `text` as a decimal integer. Below U+0080 only [`is_ascii_space`] characters around it
/// are whitespace, and at and above U+0080 the [`is_space`] characters are. A digit outside ASCII
/// is refused.
#[must_use]
pub fn int_text(text: &str) -> Option<IntText<'_>> {
    signed(text.trim_matches(is_int_space).as_bytes())
}

/// Reads `text` as [`int_text`] does, and refuses a text of more than 4300 digits, leading zeros
/// included: the most that an integer conversion of a text accepts.
#[must_use]
pub fn convertible_int(text: &str) -> Option<IntText<'_>> {
    int_text(text).filter(|found| found.digits().count() <= MAX_CONVERTIBLE_DIGITS)
}

/// Reads `bytes` as a decimal integer, with only [`is_ascii_space`] bytes around it.
#[must_use]
pub fn int_bytes(bytes: &[u8]) -> Option<IntText<'_>> {
    signed(strip_bytes(bytes))
}

fn is_int_space(c: char) -> bool {
    match u8::try_from(c) {
        Ok(byte) if byte.is_ascii() => is_ascii_space(byte),
        _ => is_space(c),
    }
}

fn signed(text: &[u8]) -> Option<IntText<'_>> {
    let (negative, grouped) = match text.split_first() {
        Some((b'-', rest)) => (true, rest),
        Some((b'+', rest)) => (false, rest),
        _ => (false, text),
    };
    let valid = grouped
        .split(|&byte| byte == b'_')
        .all(|group| !group.is_empty() && group.iter().all(u8::is_ascii_digit));
    valid.then_some(IntText { negative, grouped })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::cases;

    // The value as the reference prints it: a sign, then the digits without leading zeros.
    fn shown(found: IntText<'_>) -> String {
        let digits: String = found
            .digits()
            .map(|digit| char::from(b'0' + digit))
            .collect();
        let digits = digits.trim_start_matches('0');
        match (found.negative, digits) {
            (_, "") => "0".to_owned(),
            (true, digits) => format!("-{digits}"),
            (false, digits) => digits.to_owned(),
        }
    }

    #[test]
    fn int_text_cases() {
        let all: Vec<(String, Option<String>)> =
            cases(env!("CARGO_MANIFEST_DIR"), "text", "int_text");
        for (input, expected) in all {
            assert_eq!(int_text(&input).map(shown), expected, "input: {input:?}");
        }
    }

    #[test]
    fn int_bytes_cases() {
        let all: Vec<(Vec<u8>, Option<String>)> =
            cases(env!("CARGO_MANIFEST_DIR"), "text", "int_bytes");
        for (input, expected) in all {
            assert_eq!(int_bytes(&input).map(shown), expected, "input: {input:?}");
        }
    }

    #[test]
    fn a_text_padded_with_an_information_separator_is_refused() {
        assert_eq!(int_text("\u{1c}5"), None);
        assert_eq!(int_bytes(b"\xc2\xa05"), None);
        assert_eq!(int_text("\u{85}5").map(shown), Some("5".to_owned()));
    }
}
