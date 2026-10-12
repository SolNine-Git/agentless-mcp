//! The read side of the tag cache: the database path, the open checks, the clone block rows, the
//! receipt texts, and the discard of an unusable database.

mod discard;
mod error;
mod lookup;
mod nogit;
mod open;
mod shell;
mod status;
mod value;

#[cfg(test)]
mod tests;

pub use discard::{DiscardFailure, Discarded, discard};
pub use error::{Refusal, SqlClass, SqlError};
pub use lookup::{Lookup, RowFailure};
pub use nogit::{FileStamp, ManifestEntry, nogit_generation};
pub use open::{RowCounts, Source, TagsDb, open_source};
pub use shell::shell_quote;
pub use status::{CacheState, CacheStatus};

use std::path::{Path, PathBuf};

use kernel::Key;
use sha2::{Digest, Sha256};

/// The schema version that this build reads. A database with another version is discarded.
pub const SCHEMA_VERSION: u16 = 17;

const DATABASE_NAME: &str = "tags.db";
const KEY_LENGTH: usize = 16;

/// Returns `<cache_root>/<key>/tags.db`, where the key is the first 16 hexadecimal digits of the
/// sha256 of `repo_root`.
#[must_use]
pub fn database_path(cache_root: &Path, repo_root: &str) -> PathBuf {
    let key = sha256_hex(repo_root.as_bytes());
    cache_root.join(&key[..KEY_LENGTH]).join(DATABASE_NAME)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Key::new(Sha256::digest(bytes).into()).to_string()
}
