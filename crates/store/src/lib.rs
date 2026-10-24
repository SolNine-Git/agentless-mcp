//! The cache, agentless.db: the database path, the open checks, the receipt texts, and the
//! discard of an unusable database; the one parse that fills a row, the cache-or-parse read
//! source, the stored clone blocks, the index plan and its writer, and the ceiling sweep.

mod analysis;
mod blob;
mod discard;
mod error;
mod freshness;
mod lookup;
mod nogit;
mod open;
mod plan;
mod reader;
mod report;
mod schema;
mod shell;
mod source;
mod status;
mod sweep;
mod value;
mod warnings;
mod writer;

#[cfg(test)]
mod tests;

pub use analysis::{
    Analysis, FACTS_SCHEMA_VERSION, FACTS_VERSION, FileText, Measured, Unmeasured, analysis,
    analyze,
};
pub use blob::{Encoded, encode};
pub use discard::{DiscardFailure, Discarded, discard_facts};
pub use error::{Refusal, SqlClass, SqlError};
pub use freshness::{Freshness, IndexMeta, freshness};
pub use lookup::{Lookup, RowFailure};
pub use nogit::{FileStamp, ManifestEntry, nogit_generation};
pub use open::RowCounts;
pub use plan::{Extract, Observed, Plan, Previous, Read, Settled, plan, settle};
pub use reader::{FactsDb, Source, open_facts};
pub use report::{EvictionReport, IndexFailure, IndexReport, IndexSkip};
pub use shell::shell_quote;
pub use source::{FactSource, Served};
pub use status::{CacheState, CacheStatus};
pub use sweep::{
    CacheEntry, DEFAULT_MAX_CACHE_BYTES, MAX_CACHE_BYTES_VARIABLE, PROTECTED_SECONDS,
    enforce_cache_limit, evict_plan, max_cache_bytes,
};
pub use writer::{NewMeta, WriteError, Writer};

use std::path::{Path, PathBuf};

use kernel::Key;
use sha2::{Digest, Sha256};

pub(crate) const FACTS_DATABASE_NAME: &str = "agentless.db";
const KEY_LENGTH: usize = 16;

/// Returns `<cache_root>/<key>/agentless.db`, where the key is the first 16 hexadecimal digits of
/// the sha256 of `repo_root`.
#[must_use]
pub fn facts_database_path(cache_root: &Path, repo_root: &str) -> PathBuf {
    let key = Key::new(Sha256::digest(repo_root.as_bytes()).into()).to_string();
    cache_root
        .join(&key[..KEY_LENGTH])
        .join(FACTS_DATABASE_NAME)
}
