//! The words of a shell command line: quotes and backslash escapes as a POSIX shell reads them,
//! with no comments and no operators.

use std::fmt;

use crate::CodePointString;

/// The reason a command line has no split into words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellWordsError {
    /// A single or double quote has no closing quote.
    NoClosingQuotation,
    /// A backslash ends the command line.
    NoEscapedCharacter,
}

impl fmt::Display for ShellWordsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NoClosingQuotation => "No closing quotation",
            Self::NoEscapedCharacter => "No escaped character",
        })
    }
}

impl std::error::Error for ShellWordsError {}

/// Splits `command` at unquoted spaces, tabs, CRs and LFs, and removes quotes and escapes.
///
/// # Errors
///
/// Returns the error for a quote with no closing quote or a backslash at the end.
pub fn split_shell_words(command: &str) -> Result<Vec<String>, ShellWordsError> {
    let words = split(command.chars())?;
    Ok(words.into_iter().map(String::from_iter).collect())
}

/// Splits `command` as [`split_shell_words`] does. A lone surrogate is a character like any other.
///
/// # Errors
///
/// Returns the error for a quote with no closing quote or a backslash at the end.
pub fn split_shell_code_points(
    command: &CodePointString,
) -> Result<Vec<CodePointString>, ShellWordsError> {
    let words = split(command.code_points())?;
    Ok(words.into_iter().map(code_point_string).collect())
}

fn code_point_string(codes: Vec<u32>) -> CodePointString {
    let mut text = CodePointString::default();
    for code in codes {
        text.push(code);
    }
    text
}

// A character or a code point. The quotes, the backslash and the separators are ASCII in both.
trait Unit: Copy + PartialEq {
    fn ascii(byte: u8) -> Self;
}

impl Unit for char {
    fn ascii(byte: u8) -> Self {
        char::from(byte)
    }
}

impl Unit for u32 {
    fn ascii(byte: u8) -> Self {
        u32::from(byte)
    }
}

fn is_separator<U: Unit>(unit: U) -> bool {
    [b' ', b'\t', b'\r', b'\n']
        .into_iter()
        .any(|byte| unit == U::ascii(byte))
}

// A quote starts a word, so '' is an empty word.
fn split<U: Unit>(mut units: impl Iterator<Item = U>) -> Result<Vec<Vec<U>>, ShellWordsError> {
    let mut words = Vec::new();
    let mut word: Option<Vec<U>> = None;
    while let Some(unit) = units.next() {
        if is_separator(unit) {
            words.extend(word.take());
            continue;
        }
        let current = word.get_or_insert_with(Vec::new);
        if unit == U::ascii(b'\'') {
            single_quoted(&mut units, current)?;
        } else if unit == U::ascii(b'"') {
            double_quoted(&mut units, current)?;
        } else if unit == U::ascii(b'\\') {
            current.push(escaped(&mut units)?);
        } else {
            current.push(unit);
        }
    }
    words.extend(word);
    Ok(words)
}

// The text up to the closing single quote, read literally.
fn single_quoted<U: Unit>(
    units: &mut impl Iterator<Item = U>,
    word: &mut Vec<U>,
) -> Result<(), ShellWordsError> {
    loop {
        let unit = units.next().ok_or(ShellWordsError::NoClosingQuotation)?;
        if unit == U::ascii(b'\'') {
            return Ok(());
        }
        word.push(unit);
    }
}

// The text up to the closing double quote. A backslash escapes only a backslash or a double
// quote; before any other character it stays.
fn double_quoted<U: Unit>(
    units: &mut impl Iterator<Item = U>,
    word: &mut Vec<U>,
) -> Result<(), ShellWordsError> {
    loop {
        let unit = units.next().ok_or(ShellWordsError::NoClosingQuotation)?;
        if unit == U::ascii(b'"') {
            return Ok(());
        }
        if unit == U::ascii(b'\\') {
            let next = escaped(units)?;
            if next != U::ascii(b'\\') && next != U::ascii(b'"') {
                word.push(unit);
            }
            word.push(next);
        } else {
            word.push(unit);
        }
    }
}

fn escaped<U: Unit>(units: &mut impl Iterator<Item = U>) -> Result<U, ShellWordsError> {
    units.next().ok_or(ShellWordsError::NoEscapedCharacter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::cases;

    #[test]
    fn split_shell_words_cases() {
        let all: Vec<(String, Option<Vec<String>>, Option<String>)> =
            cases(env!("CARGO_MANIFEST_DIR"), "text", "shell_split");
        for (command, words, error) in all {
            let found = match split_shell_words(&command) {
                Ok(split) => (Some(split), None),
                Err(refused) => (None, Some(refused.to_string())),
            };
            assert_eq!(found, (words, error), "command {command:?}");
        }
    }

    #[test]
    fn the_code_point_split_matches_the_text_split() {
        let all: Vec<(String, Option<Vec<String>>, Option<String>)> =
            cases(env!("CARGO_MANIFEST_DIR"), "text", "shell_split");
        for (command, words, error) in all {
            let found = match split_shell_code_points(&CodePointString::from(command.as_str())) {
                Ok(split) => (Some(split), None),
                Err(refused) => (None, Some(refused.to_string())),
            };
            let words = words.map(|words| words.iter().map(|w| w.as_str().into()).collect());
            assert_eq!(found, (words, error), "command {command:?}");
        }
    }

    #[test]
    fn a_lone_surrogate_stays_in_its_word() {
        let mut command = CodePointString::from("rg 'a ");
        command.push(0xd800);
        command.push_str("' b");
        let words = split_shell_code_points(&command).expect("the quotes close");
        let mut quoted = CodePointString::from("a ");
        quoted.push(0xd800);
        assert_eq!(words, ["rg".into(), quoted, "b".into()]);
    }
}
