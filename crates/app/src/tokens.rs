//! The token counters that every budget and the output ceiling measure text with.

use tiktoken_rs::CoreBPE;

use crate::Error;

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

/// The `cl100k_base` encoding: the count of the tokens it encodes a text into, with special
/// tokens read as plain text.
pub struct Cl100k {
    encoding: CoreBPE,
}

impl Cl100k {
    /// Loads the encoding, which is part of the binary.
    ///
    /// # Errors
    ///
    /// Returns an operation failure when the encoding does not load.
    pub fn new() -> Result<Cl100k, Error> {
        let encoding = tiktoken_rs::cl100k_base().map_err(|error| {
            let message = format!("--token-counter tiktoken could not load cl100k_base: {error}");
            kernel::Error::OperationFailed(message)
        })?;
        Ok(Cl100k { encoding })
    }
}

impl TokenCounter for Cl100k {
    fn count(&self, text: &str) -> usize {
        self.encoding.encode_ordinary(text).len()
    }
}

#[cfg(test)]
mod tests;
