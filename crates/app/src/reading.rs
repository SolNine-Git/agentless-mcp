//! What a read operation reads from a session: the repository, its git state and its fact source,
//! and the deadline of the call it serves.

use kernel::Deadline;
use store::FactSource;

use crate::{Repo, Session};

/// The repository, its git state and its fact source, as one read operation reads them, and the
/// call's deadline.
#[derive(Clone, Copy, Debug)]
pub struct Reading<'a> {
    /// The repository.
    pub repo: &'a Repo,
    /// HEAD, the tree, the dirty count, and the note on what is unknown.
    pub snapshot: &'a git::Snapshot,
    /// The source of each file's facts: the cache rows, or a parse.
    pub facts: &'a FactSource,
    /// The deadline that the long loops check, or `None` for a read with no time limit.
    pub deadline: Option<&'a Deadline>,
}

impl<'a> Reading<'a> {
    /// Returns the parts of `session` that a read operation reads, with no deadline.
    #[must_use]
    pub fn of(session: &'a Session) -> Reading<'a> {
        Reading {
            repo: &session.repo,
            snapshot: &session.snapshot,
            facts: &session.facts,
            deadline: None,
        }
    }

    /// Returns this reading with `deadline` as the call's deadline.
    #[must_use]
    pub fn with_deadline(self, deadline: &'a Deadline) -> Reading<'a> {
        Reading {
            deadline: Some(deadline),
            ..self
        }
    }
}
