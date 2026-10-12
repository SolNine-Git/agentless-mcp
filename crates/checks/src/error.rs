//! The failures that stop a check run, and the failures of one measured side.

/// Why one side of a pair has no measurement.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SideError {
    /// The parser gave no usable tree.
    #[error("{0}")]
    Unparsable(syntax::Unparsable),
    /// The tree does not measure.
    #[error("{0}")]
    Unmeasurable(metrics::Unmeasurable),
    /// A block source holds the reason, as the caller-facing text it stored.
    #[error("{0}")]
    Recorded(String),
}

/// A failure that stops a check run: bad input or data, never a property of the change.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Two pairs of one run name the same path.
    #[error("each path may appear in only one text pair of a run: {path}")]
    RepeatedPath {
        /// The repeated path.
        path: String,
    },
    /// The clone index got a path that is not tier-1 source.
    #[error("{path} is not a tier-1 source file, so clone_delta cannot index it")]
    NotIndexable {
        /// The path.
        path: String,
    },
    /// A side failed in a way that no check reports as a not-checked row.
    #[error("{path}: {source}")]
    Side {
        /// The path of the pair or the index file.
        path: String,
        /// The failure.
        source: SideError,
    },
}
