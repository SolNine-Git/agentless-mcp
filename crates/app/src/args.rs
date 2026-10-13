//! The integer a caller passes to a read operation, kept as written until the operation checks it.

use std::fmt;

// An int() text with more digits than this is refused before any conversion.
const MAX_DIGITS: usize = 4300;

/// An integer argument as the caller wrote it, of any size. It displays as its decimal value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntArg {
    negative: bool,
    // The decimal digits without leading zeros, and "0" for zero.
    digits: String,
}

impl IntArg {
    /// Reads `text` by the decimal integer rule of [`text::int_text`], or returns `None`. A text
    /// with more than 4300 digits is refused.
    #[must_use]
    pub fn parse(text: &str) -> Option<IntArg> {
        let found = text::int_text(text)?;
        if found.digits().count() > MAX_DIGITS {
            return None;
        }
        let digits: String = found
            .digits()
            .skip_while(|&digit| digit == 0)
            .map(|digit| char::from(b'0' + digit))
            .collect();
        if digits.is_empty() {
            return Some(IntArg::from_usize(0));
        }
        Some(IntArg {
            negative: found.negative,
            digits,
        })
    }

    /// Returns the argument whose value is `value`.
    #[must_use]
    pub fn from_usize(value: usize) -> IntArg {
        IntArg {
            negative: false,
            digits: value.to_string(),
        }
    }

    /// Returns the value, saturated at `usize::MAX`, or `None` when it is negative.
    #[must_use]
    pub fn value(&self) -> Option<usize> {
        if self.negative {
            return None;
        }
        Some(self.digits.parse().unwrap_or(usize::MAX))
    }
}

impl fmt::Display for IntArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        f.write_str(&self.digits)
    }
}

#[cfg(test)]
mod tests;
