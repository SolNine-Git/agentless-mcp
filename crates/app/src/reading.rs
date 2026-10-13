//! What a read operation reads from a session: the repository, its git state and its fact source.

use store::FactSource;

use crate::{Repo, Session};

/// The repository, its git state and its fact source, as one read operation reads them.
#[derive(Clone, Copy, Debug)]
pub struct Reading<'a> {
    /// The repository.
    pub repo: &'a Repo,
    /// HEAD, the tree, the dirty count, and the note on what is unknown.
    pub snapshot: &'a git::Snapshot,
    /// The source of each file's facts: the cache rows, or a parse.
    pub facts: &'a FactSource,
}

impl<'a> Reading<'a> {
    /// Returns the parts of `session` that a read operation reads.
    #[must_use]
    pub fn of(session: &'a Session) -> Reading<'a> {
        Reading {
            repo: &session.repo,
            snapshot: &session.snapshot,
            facts: &session.facts,
        }
    }
}
