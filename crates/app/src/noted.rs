//! A value with the warnings that arose on the way to it, which a caller prints before it.

use crate::Error;

/// A value and the warnings to print on stderr, in the order they arose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Noted<T> {
    /// The value.
    pub value: T,
    /// The warnings.
    pub diagnostics: Vec<String>,
}

impl<T> Noted<T> {
    /// Returns `value` with no warnings.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            value,
            diagnostics: Vec::new(),
        }
    }

    /// Maps the value and keeps the warnings.
    #[must_use]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Noted<U> {
        Noted {
            value: f(self.value),
            diagnostics: self.diagnostics,
        }
    }
}

impl<T> Noted<Result<T, Error>> {
    // Maps the value when there is one, and keeps the warnings.
    pub(crate) fn map_ok<U>(self, f: impl FnOnce(T) -> U) -> Noted<Result<U, Error>> {
        self.map(|value| value.map(f))
    }

    // Runs `next` on the value when there is one: the warnings of both, in order.
    pub(crate) fn then_ok<U>(
        self,
        next: impl FnOnce(T) -> Noted<Result<U, Error>>,
    ) -> Noted<Result<U, Error>> {
        let Noted {
            value,
            mut diagnostics,
        } = self;
        let value = match value {
            Ok(value) => {
                let second = next(value);
                diagnostics.extend(second.diagnostics);
                second.value
            }
            Err(error) => Err(error),
        };
        Noted { value, diagnostics }
    }
}
