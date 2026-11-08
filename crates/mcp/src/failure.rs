//! Why a call gave no answer: a refusal written for the caller, or a failure the server did not
//! plan for, whose detail goes to the log only.

use prompts::messages::WALK_REFUSED_MCP;

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
            app::Error::Kind(kernel::Error::WalkBoundExceeded(reason)) => {
                Failure::Refused(WALK_REFUSED_MCP.render(&reason))
            }
            app::Error::Kind(kind) => Failure::Refused(kind.to_string()),
            other => Failure::Unplanned(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Failure;

    #[test]
    fn a_walk_bound_refusal_names_what_a_caller_of_the_server_can_do() {
        let reason = "walk refused: more than 2 files under /r".to_owned();
        let refused = Failure::from(app::Error::Kind(kernel::Error::WalkBoundExceeded(reason)));
        let expected = "walk refused: more than 2 files under /r. No argument changes a walk bound. \
                        Serve a subdirectory with --root, and the walk covers only that subtree.";
        assert_eq!(refused, Failure::Refused(expected.to_owned()));
        let other = kernel::Error::RepoResolution("not a directory: /r".to_owned());
        let other = Failure::from(app::Error::Kind(other));
        assert_eq!(other, Failure::Refused("not a directory: /r".to_owned()));
    }
}
