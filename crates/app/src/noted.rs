//! A value with the warnings that arose on the way to it, which a caller prints before it.

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
