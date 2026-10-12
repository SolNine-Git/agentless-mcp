//! The error an operation returns.

/// Why an operation gave no answer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A domain refusal or failure, with its kind and message.
    #[error(transparent)]
    Kind(#[from] kernel::Error),
    /// A pair check failed on input it cannot read: a defect, never a property of the change.
    #[error(transparent)]
    Check(#[from] checks::Error),
    /// A row count of the tag cache failed after the cache opened.
    #[error(transparent)]
    Store(#[from] store::SqlError),
}
