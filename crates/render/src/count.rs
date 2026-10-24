//! The output ceiling: the token count that no answer may pass, and the counter that measures
//! text against it.

use app::TokenCounter;

pub(crate) struct Ceiling<'a> {
    pub(crate) counter: &'a dyn TokenCounter,
    pub(crate) tokens: usize,
}

impl<'a> Ceiling<'a> {
    // The answer ceiling. It saturates, so a target with a narrow usize keeps all text.
    pub(crate) fn standard(counter: &'a dyn TokenCounter) -> Self {
        Self {
            counter,
            tokens: usize::try_from(app::MAX_OUTPUT_TOKENS).unwrap_or(usize::MAX),
        }
    }

    pub(crate) fn count(&self, text: &str) -> usize {
        self.counter.count(text)
    }
}
