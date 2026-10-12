//! A step that stops the gate before the decision: the outcome it logs and why.

use crate::record::Outcome;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Halt {
    pub(crate) outcome: Outcome,
    pub(crate) reason: String,
    // The line that stderr gets under the internal-error line, for an internal error only.
    pub(crate) internal: Option<String>,
}

impl Halt {
    pub(crate) fn skipped(reason: impl Into<String>) -> Self {
        Self::stopped(Outcome::Skipped, reason.into())
    }

    pub(crate) fn degraded(reason: impl Into<String>) -> Self {
        Self::stopped(Outcome::Degraded, reason.into())
    }

    pub(crate) fn internal(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            internal: Some(detail.clone()),
            ..Self::degraded(format!("internal error: {detail}"))
        }
    }

    pub(crate) fn malformed(problem: impl std::fmt::Display) -> Self {
        Self::degraded(format!("malformed payload: {problem}"))
    }

    fn stopped(outcome: Outcome, reason: String) -> Self {
        Self {
            outcome,
            reason,
            internal: None,
        }
    }
}
