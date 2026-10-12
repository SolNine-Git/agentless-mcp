//! The generation of a tree outside git: a digest of its sorted file manifest.

use kernel::Key;
use sha2::{Digest, Sha256};

const PREFIX: &str = "nogit:";
const DIGEST_LENGTH: usize = 16;

/// One walked file of a tree outside git, with its stat when the stat succeeded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntry {
    /// The path relative to the root, as the walk gives it.
    pub path: String,
    /// The size and modification time, or `None` when the stat failed.
    pub stamp: Option<FileStamp>,
}

/// The size and the modification time of one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    /// The size in bytes.
    pub size: u64,
    /// The modification time in nanoseconds since the epoch.
    pub mtime_ns: i128,
}

/// Returns `nogit:` and the first 16 hexadecimal digits of the sha256 over the manifest, in the
/// order given.
#[must_use]
pub fn nogit_generation(manifest: &[ManifestEntry]) -> String {
    let mut digest = Sha256::new();
    for entry in manifest {
        let line = match entry.stamp {
            Some(stamp) => format!("{}\0{}\0{}\n", entry.path, stamp.size, stamp.mtime_ns),
            None => format!("{}\0missing\n", entry.path),
        };
        digest.update(line);
    }
    let hex = Key::new(digest.finalize().into()).to_string();
    format!("{PREFIX}{}", &hex[..DIGEST_LENGTH])
}
