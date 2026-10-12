//! The git reads the tool makes: a hardened bounded runner, the repository snapshot, the change
//! listing parsers, and the repository walk.

mod changes;
mod run;
mod snapshot;
mod toplevel;
mod walk;

#[cfg(test)]
mod testing;

pub use changes::{
    BINARY_PROBE_BYTES, COMPARED_STATUSES, Change, MAX_LISTING_BYTES, Malformed, Side, UNTRACKED,
    looks_binary, parse_changes, parse_commit, parse_untracked,
};
pub use run::{Failure, Git, GitOutcome, Unfinished};
pub use snapshot::Snapshot;
pub use toplevel::Toplevel;
pub use walk::{RepoFile, Walk, WalkError, WalkWarning};

use run::Limits;
