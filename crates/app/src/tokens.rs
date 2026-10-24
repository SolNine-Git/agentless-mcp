//! The token counters that every budget and the output ceiling measure text with.

use std::fmt::Display;
use std::sync::OnceLock;

use tiktoken_rs::CoreBPE;

use crate::Error;

// One copy lives as long as the process: a free of its tables at the end of each command costs
// time and gives nothing back.
static CL100K: OnceLock<CoreBPE> = OnceLock::new();

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
    encoding: &'static CoreBPE,
}

impl Cl100k {
    /// Returns a counter over the process's one copy of the encoding, which the first call loads
    /// from the binary.
    ///
    /// # Errors
    ///
    /// Returns an operation failure when the encoding does not load.
    pub fn new() -> Result<Cl100k, Error> {
        let encoding = loaded_once(&CL100K, tiktoken_rs::cl100k_base)?;
        Ok(Cl100k { encoding })
    }
}

// The encoding in `lock`, which `load` fills on the first call. A failed load leaves the lock
// empty, so the next call loads again.
fn loaded_once<E: Display>(
    lock: &OnceLock<CoreBPE>,
    load: impl FnOnce() -> Result<CoreBPE, E>,
) -> Result<&CoreBPE, Error> {
    if let Some(encoding) = lock.get() {
        return Ok(encoding);
    }
    let loaded = load().map_err(|error| {
        let message = format!("--token-counter tiktoken could not load cl100k_base: {error}");
        kernel::Error::OperationFailed(message)
    })?;
    Ok(lock.get_or_init(|| loaded))
}

impl TokenCounter for Cl100k {
    fn count(&self, text: &str) -> usize {
        self.encoding.encode_ordinary(text).len()
    }
}

#[cfg(test)]
mod tests;
