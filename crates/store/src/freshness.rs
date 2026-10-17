//! Whether a stored index serves the repository now: the admission of its meta row, and the
//! comparison of its generation with the repository's.

use std::cmp::Ordering;

use crate::{FACTS_SCHEMA_VERSION, Refusal, analysis};

/// The meta row of an index: its schema version, its repository, its generation, and the
/// analysis its rows were made under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexMeta {
    /// The schema version that the database records.
    pub schema: i64,
    /// The repository root that the database records.
    pub repo_root: String,
    /// The generation that the index was built at.
    pub generation: String,
    /// The analysis digest of the rows, or `None` for a tags.db, whose meta row has none.
    pub analysis: Option<Vec<u8>>,
}

/// What an agentless.db index means for the repository now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// The index cannot serve the repository.
    Refused(Refusal),
    /// The index was built at the repository's generation now.
    Current,
    /// The index was built at another generation.
    Stale,
}

/// Returns whether the agentless.db meta row `meta` serves `repo_root` at `generation`. No meta
/// row, another schema version, another repository and another analysis are refused, in that
/// order.
#[must_use]
pub fn freshness(meta: Option<&IndexMeta>, repo_root: &str, generation: &str) -> Freshness {
    let Some(meta) = meta else {
        return Freshness::Refused(Refusal::NoIndex);
    };
    let refused = facts_schema_rejection(meta.schema)
        .or_else(|| rejection(meta, FACTS_SCHEMA_VERSION, repo_root))
        .or_else(|| analysis_rejection(meta));
    if let Some(refusal) = refused {
        return Freshness::Refused(refusal);
    }
    if meta.generation == generation {
        Freshness::Current
    } else {
        Freshness::Stale
    }
}

// The schema rule of agentless.db alone: a newer layout belongs to the newer build that wrote it,
// so it is kept. tags.db keeps its own rule, which discards a newer layout too.
pub(crate) fn facts_schema_rejection(found: i64) -> Option<Refusal> {
    let expected = FACTS_SCHEMA_VERSION;
    match found.cmp(&i64::from(expected)) {
        Ordering::Less => Some(Refusal::Schema { found, expected }),
        Ordering::Equal => None,
        Ordering::Greater => Some(Refusal::NewerSchema { found, expected }),
    }
}

// Rows of another analysis are never served, so an index of one is not current. The next index
// run rewrites every row.
pub(crate) fn analysis_rejection(meta: &IndexMeta) -> Option<Refusal> {
    let current = analysis();
    let made_here = meta.analysis.as_deref() == Some(current.bytes().as_slice());
    (!made_here).then_some(Refusal::OtherAnalysis)
}

// The schema test comes first: a database of another schema may not even name its repository.
pub(crate) fn rejection(meta: &IndexMeta, expected: u16, repo_root: &str) -> Option<Refusal> {
    if meta.schema != i64::from(expected) {
        let found = meta.schema;
        return Some(Refusal::Schema { found, expected });
    }
    (meta.repo_root != repo_root).then(|| Refusal::OtherRepository {
        repo_root: meta.repo_root.clone(),
    })
}
