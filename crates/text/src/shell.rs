//! The words of a shell command line: quotes and backslash escapes as a POSIX shell reads them,
//! with no comments and no operators.

use std::fmt;
use std::str::Chars;

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
    let mut words = Vec::new();
    let mut rest = command.trim_start_matches(is_separator);
    while !rest.is_empty() {
        let (word, after) = word_at(rest)?;
        words.push(word);
        rest = after.trim_start_matches(is_separator);
    }
    Ok(words)
}

fn is_separator(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

// The word at the start of `text`, and the text after the separator that ends the word.
fn word_at(text: &str) -> Result<(String, &str), ShellWordsError> {
    let mut word = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => word.push_str(&single_quoted(&mut chars)?),
            '"' => word.push_str(&double_quoted(&mut chars)?),
            '\\' => word.push(escaped(&mut chars)?),
            c if is_separator(c) => break,
            c => word.push(c),
        }
    }
    Ok((word, chars.as_str()))
}

// The text up to the closing single quote, read literally.
fn single_quoted(chars: &mut Chars<'_>) -> Result<String, ShellWordsError> {
    let mut quoted = String::new();
    loop {
        match chars.next().ok_or(ShellWordsError::NoClosingQuotation)? {
            '\'' => return Ok(quoted),
            c => quoted.push(c),
        }
    }
}

// The text up to the closing double quote. A backslash escapes only a backslash or a double
// quote; before any other character it stays.
fn double_quoted(chars: &mut Chars<'_>) -> Result<String, ShellWordsError> {
    let mut quoted = String::new();
    loop {
        match chars.next().ok_or(ShellWordsError::NoClosingQuotation)? {
            '"' => return Ok(quoted),
            '\\' => {
                let c = escaped(chars)?;
                if !matches!(c, '\\' | '"') {
                    quoted.push('\\');
                }
                quoted.push(c);
            }
            c => quoted.push(c),
        }
    }
}

fn escaped(chars: &mut Chars<'_>) -> Result<char, ShellWordsError> {
    chars.next().ok_or(ShellWordsError::NoEscapedCharacter)
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
}
