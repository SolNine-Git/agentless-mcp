//! The token estimate that every bound in an answer uses: one token per four characters.

pub(crate) fn tokens(text: &str) -> usize {
    chars_to_tokens(text.chars().count())
}

// Floor division, so a text of fewer than four characters costs nothing.
pub(crate) const fn chars_to_tokens(chars: usize) -> usize {
    chars / 4
}

// The answer ceiling as a count. It saturates, so a target with a narrow usize keeps all text.
pub(crate) fn ceiling() -> usize {
    usize::try_from(app::MAX_OUTPUT_TOKENS).unwrap_or(usize::MAX)
}
