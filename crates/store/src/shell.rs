//! Shell quoting for text that a POSIX shell reads as one word.

/// Returns `text` as one shell word: unchanged when each character is an ASCII letter or digit
/// or one of `_@%+=:,./-`, else in single quotes with each single quote written as `'"'"'`.
#[must_use]
pub fn shell_quote(text: &str) -> String {
    if text.is_empty() {
        return "''".to_owned();
    }
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c))
    {
        return text.to_owned();
    }
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use text::testing::check_pairs;

    #[test]
    fn shell_quote_cases() {
        check_pairs(
            env!("CARGO_MANIFEST_DIR"),
            "shell",
            "shlex_quote",
            shell_quote,
        );
    }
}
