//! The token counter that every budget and the output ceiling measure text with.

/// Counts the tokens of a text.
pub trait TokenCounter: Send + Sync {
    /// Returns the token count of `text`.
    fn count(&self, text: &str) -> usize;
}

/// The default estimate: one token per four code points, rounded down.
#[derive(Clone, Copy, Debug, Default)]
pub struct Chars4;

impl TokenCounter for Chars4 {
    fn count(&self, text: &str) -> usize {
        text.chars().count() / 4
    }
}

#[cfg(test)]
mod tests {
    use super::{Chars4, TokenCounter};

    #[test]
    fn chars4_counts_code_points_and_rounds_down() {
        assert_eq!(Chars4.count(""), 0);
        assert_eq!(Chars4.count("abc"), 0);
        assert_eq!(Chars4.count("abcd"), 1);
        // Four code points in sixteen bytes are one token, not four.
        assert_eq!(Chars4.count("\u{1f600}\u{1f600}\u{1f600}\u{1f600}"), 1);
        assert_eq!(Chars4.count(&"x".repeat(4001)), 1000);
    }
}
