use std::path::{Path, PathBuf};

use super::cases::cases;
use crate::{
    CacheState, CacheStatus, FileStamp, ManifestEntry, facts_database_path, nogit_generation,
};

#[test]
fn each_indexed_status_gives_the_reference_receipt() {
    for (generation, repo_generation, repo_root, receipt, matches) in cases().receipts {
        let status = CacheStatus {
            path: PathBuf::from("/agentless-store-fixture/cache-home/agentless.db"),
            state: CacheState::Indexed {
                generation,
                repo_generation,
                repo_root,
            },
        };
        assert_eq!(
            (status.receipt(), status.generation_matches()),
            (receipt, matches)
        );
        assert_eq!(status.note(), "");
    }
}

// The reference names its tags.db in the same key directory (the D7 deviation).
#[test]
fn each_repository_root_gives_the_reference_database_path() {
    for (cache_root, repo_root, expected) in cases().keys {
        let found = facts_database_path(Path::new(&cache_root), &repo_root);
        let expected = PathBuf::from(expected).with_file_name("agentless.db");
        assert_eq!(found, expected, "{repo_root}");
    }
}

#[test]
fn each_manifest_gives_the_reference_generation() {
    for (rows, expected) in cases().nogit {
        let manifest: Vec<ManifestEntry> = rows
            .into_iter()
            .map(|(path, stamp)| ManifestEntry {
                path,
                stamp: stamp.map(|(size, mtime_ns)| FileStamp { size, mtime_ns }),
            })
            .collect();
        assert_eq!(nogit_generation(&manifest), expected);
    }
}
