//! The cache status of one call, and the receipt and note texts that describe it.

use std::path::PathBuf;

use prompts::messages::{CACHE_ABSENT_REFRESHING, CACHE_STALE_REFRESHING, CACHE_STALE_REMEDIATION};

use crate::{Refusal, shell_quote};

/// What the call found in the cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheState {
    /// The caller asked to parse every file itself.
    Bypassed,
    /// No database is used, with the refusal when a database exists.
    Absent {
        /// Why an existing database is not used.
        note: Option<Refusal>,
    },
    /// A database is used.
    Indexed {
        /// The generation that the database was built at.
        generation: String,
        /// The generation of the repository now.
        repo_generation: String,
        /// The repository root, which the remediation names.
        repo_root: String,
    },
}

/// The database path of one call and what the call found there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheStatus {
    /// The database path of the repository, whether or not a file exists there.
    pub path: PathBuf,
    /// What the call found.
    pub state: CacheState,
}

impl CacheStatus {
    /// Returns the `cache:` field of the receipt when no background refresh runs.
    #[must_use]
    pub fn receipt(&self) -> String {
        self.receipt_while(false)
    }

    /// Returns the `cache:` field of the receipt. `refreshing` says that a background refresh of
    /// this database runs, which the field names in place of the remediation.
    #[must_use]
    pub fn receipt_while(&self, refreshing: bool) -> String {
        match &self.state {
            CacheState::Bypassed => "bypassed (--no-cache)".to_owned(),
            CacheState::Absent { note } => absent(note.as_ref(), refreshing),
            CacheState::Indexed {
                generation,
                repo_generation,
                ..
            } if generation == repo_generation => format!("g:{generation} fresh"),
            CacheState::Indexed {
                generation,
                repo_generation,
                repo_root,
            } => {
                let remediation = if refreshing {
                    CACHE_STALE_REFRESHING.to_owned()
                } else {
                    CACHE_STALE_REMEDIATION.render(&shell_quote(repo_root))
                };
                format!(
                    "g:{generation} generation mismatch (repo g:{repo_generation}); {remediation}"
                )
            }
        }
    }

    /// Returns the refusal note, or an empty text when nothing was refused.
    #[must_use]
    pub fn note(&self) -> String {
        match &self.state {
            CacheState::Absent { note: Some(note) } => note.to_string(),
            _ => String::new(),
        }
    }

    /// Returns true when a database is used and was built at the repository's generation.
    #[must_use]
    pub fn generation_matches(&self) -> bool {
        matches!(
            &self.state,
            CacheState::Indexed { generation, repo_generation, .. } if generation == repo_generation
        )
    }
}

// The refusal note and the refresh notice, joined with "; ", in parentheses after "none".
fn absent(note: Option<&Refusal>, refreshing: bool) -> String {
    let note = note.map(ToString::to_string);
    let notes: Vec<&str> = [
        note.as_deref(),
        refreshing.then_some(CACHE_ABSENT_REFRESHING),
    ]
    .into_iter()
    .flatten()
    .collect();
    if notes.is_empty() {
        return "none".to_owned();
    }
    format!("none ({})", notes.join("; "))
}
