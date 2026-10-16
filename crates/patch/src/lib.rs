//! Candidate edits: SEARCH/REPLACE blocks, unified diffs, their application, keys, and the vote.

mod apply;
mod blocks;
mod delta;
mod elision;
mod header;
mod keys;
mod natural;
mod unidiff;
mod vote;

pub use apply::{ApplyResult, EditOutcome, EditStatus, apply_edits};
pub use blocks::{BlockError, Edit, ParseResult, parse_blocks};
pub use delta::{SyntaxVerdict, syntax_delta};
pub use elision::{Elided, Located, locate, resolve_elisions};
pub use keys::{TEXT_ONLY_KEY_PREFIX, equivalence_key, file_key, normalized_stream};
pub use unidiff::{DiffNote, DiffParse, orientation, parse_unified_diff};
pub use vote::{Cluster, Exclusion, Regression, Tier, VoteCandidate, VoteReport, rank};

/// The most edits that one request applies.
pub const MAX_EDITS: usize = 500;
/// The most bytes of search and replace text that one request applies.
pub const MAX_EDIT_BYTES: usize = 2_000_000;

// Python's repr of a str: the quote it picks, and an escape for each quote, backslash and
// character that is not printable.
fn quoted(text: &str) -> String {
    text::CodePointString::from(text).quoted()
}

fn trim_end(text: &str) -> &str {
    text.trim_end_matches(text::is_space)
}
