//! What one index run did: the numbers of its summary line and of its JSON form.

use std::path::PathBuf;

use serde::{Serialize, Serializer};

/// One file that the run could not record, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IndexFailure {
    /// The path below the repository root.
    pub path: String,
    /// The reason.
    pub reason: String,
}

/// One file of a known language that the run declined to read, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct IndexSkip {
    /// The path below the repository root.
    pub path: String,
    /// The reason.
    pub reason: String,
}

/// What one sweep of the cache root deleted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EvictionReport {
    /// The deleted databases.
    pub databases: u64,
    /// The bytes that the deletes reclaimed.
    pub size: u64,
}

/// What one index run did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexReport {
    /// The database that the run wrote.
    pub database: PathBuf,
    /// The generation that the run recorded.
    pub generation: String,
    /// The files that the run extracted.
    pub indexed: usize,
    /// The files whose rows the run kept.
    pub reused: usize,
    /// The recorded files that left the repository.
    pub pruned: usize,
    /// The files that the database holds after the run.
    pub files: u64,
    /// The symbols that the database holds.
    pub tags: u64,
    /// The import statements that the database holds.
    pub imports: u64,
    /// The identifier occurrences that the database holds.
    pub refs: u64,
    /// Each file that the run could not record, in walk order.
    pub failures: Vec<IndexFailure>,
    /// Each file that the run declined to read, in walk order.
    pub skipped_files: Vec<IndexSkip>,
    /// What the ceiling sweep deleted after the run.
    pub evicted: EvictionReport,
}

impl IndexReport {
    /// Returns the count of files that the run could not record.
    #[must_use]
    pub fn errors(&self) -> usize {
        self.failures.len()
    }

    /// Returns the count of files that the run declined to read.
    #[must_use]
    pub fn skipped(&self) -> usize {
        self.skipped_files.len()
    }

    /// Returns the one-line summary. The eviction clause appears only when the sweep deleted a
    /// database.
    #[must_use]
    pub fn summary_line(&self) -> String {
        format!(
            "indexed {}, reused {}, pruned {}, skipped {}, errors {}: {} files, {} tags, {} \
             imports, {} refs at g:{} in {}{}",
            self.indexed,
            self.reused,
            self.pruned,
            self.skipped(),
            self.errors(),
            self.files,
            self.tags,
            self.imports,
            self.refs,
            self.generation,
            self.database.display(),
            eviction_clause(self.evicted)
        )
    }
}

fn eviction_clause(evicted: EvictionReport) -> String {
    if evicted.databases == 0 {
        return String::new();
    }
    let noun = if evicted.databases == 1 {
        "repository"
    } else {
        "repositories"
    };
    format!(
        ", evicted {} cached {noun} ({} bytes)",
        evicted.databases, evicted.size
    )
}

// The JSON form, in its key order. `evicted` is always present.
#[derive(Serialize)]
struct Json<'a> {
    database: String,
    generation: &'a str,
    indexed: usize,
    reused: usize,
    pruned: usize,
    evicted: Evicted,
    errors: usize,
    files: u64,
    tags: u64,
    imports: u64,
    refs: u64,
    skipped: usize,
    failures: &'a [IndexFailure],
    skipped_files: &'a [IndexSkip],
}

#[derive(Serialize)]
struct Evicted {
    databases: u64,
    bytes: u64,
}

impl Serialize for IndexReport {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        Json {
            database: self.database.display().to_string(),
            generation: &self.generation,
            indexed: self.indexed,
            reused: self.reused,
            pruned: self.pruned,
            evicted: Evicted {
                databases: self.evicted.databases,
                bytes: self.evicted.size,
            },
            errors: self.errors(),
            files: self.files,
            tags: self.tags,
            imports: self.imports,
            refs: self.refs,
            skipped: self.skipped(),
            failures: &self.failures,
            skipped_files: &self.skipped_files,
        }
        .serialize(serializer)
    }
}
