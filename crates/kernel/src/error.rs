/// Every error the tool raises: a variant names the kind, and its string is the message to show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The operation could not be carried out as the caller asked for it.
    #[error("{0}")]
    OperationFailed(String),
    /// A file the caller named could not be read.
    #[error("{0}")]
    InputUnreadable(String),
    /// A path or argument escapes the allowed root.
    #[error("{0}")]
    SecurityRefusal(String),
    /// A traversal hit one of its bounds: depth, files, or bytes.
    #[error("{0}")]
    WalkBoundExceeded(String),
    /// A grammar cannot be loaded.
    #[error("{0}")]
    LanguageUnavailable(String),
    /// A source text has no metrics.
    #[error("{0}")]
    UnmeasurableSource(String),
    /// A repository root could not be resolved or read.
    #[error("{0}")]
    RepoResolution(String),
    /// A revision does not resolve to a commit.
    #[error("{0}")]
    RevisionUnresolved(String),
    /// A call stopped at a checkpoint: its time ran out, or its client left.
    #[error("{0}")]
    CallStopped(String),
    /// Another process holds the cache's write lock.
    #[error("{0}")]
    CacheLocked(String),
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn an_error_displays_its_message_alone() {
        let error = Error::WalkBoundExceeded("walk refused: more than 3 files".to_owned());
        assert_eq!(error.to_string(), "walk refused: more than 3 files");
    }
}
