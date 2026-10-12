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

/// Why a contract rule cannot run.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RuleProblem {
    /// The query does not compile against the rule's grammar.
    #[error("does not compile: {0}")]
    Compile(syntax::QueryError),
    /// The query does not capture `@hit`, or it has another capture without a `_` prefix.
    #[error("must capture @hit, and every other capture must start with _")]
    Captures,
    /// The query uses predicates outside the allowed set.
    #[error("uses predicates outside the allowed set: {}", .0.join(", "))]
    Predicates(Vec<String>),
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
    /// A contract rule cannot run.
    #[error("contract rule {id} for {language} {problem}")]
    Rule {
        /// The rule id.
        id: &'static str,
        /// The rule's language.
        language: &'static str,
        /// What is wrong with the rule.
        problem: RuleProblem,
    },
    /// A contract rule's query failed on one tree.
    #[error("contract rule {id} for {language} failed on {path}: {source}")]
    Query {
        /// The rule id.
        id: &'static str,
        /// The rule's language.
        language: &'static str,
        /// The path of the text the query ran on.
        path: String,
        /// The failure.
        source: syntax::QueryRunError,
    },
}
