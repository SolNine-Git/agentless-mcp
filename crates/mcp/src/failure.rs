//! Why a call gave no answer: a refusal written for the caller, or a failure the server did not
//! plan for, whose detail goes to the log only.

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    // The text names the fix; the caller reads it after the tool's prefix.
    Refused(String),
    // The text is for the log; the caller reads the unplanned-error sentence.
    Unplanned(String),
}

impl From<app::Error> for Failure {
    fn from(error: app::Error) -> Failure {
        match error {
            app::Error::Kind(kind) => Failure::Refused(kind.to_string()),
            other => Failure::Unplanned(other.to_string()),
        }
    }
}
