//! The clone index that `clone_delta` compares against: the repository's blocks and its gaps.

use std::collections::BTreeMap;

use metrics::{Block, CLONE_REASONS};
use prompts::messages;
use syntax::Language;

use crate::error::{Error, SideError};
use crate::pair::{pair_language, side_reason};

/// The clone index of one run: the indexed repository, or none because the run has no tag cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloneIndex {
    /// The hook form found no tag cache, so the run reads no repository file.
    NoTagCache,
    /// The repository files the index holds, and why each other one is missing.
    Built(IndexedFiles),
}

/// The blocks of each indexed file, the reason for each file left out, and a listing failure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexedFiles {
    /// The clone blocks of each indexed file, by repo-relative path.
    pub files: BTreeMap<String, Vec<Block>>,
    /// Why each other file is not indexed, by repo-relative path.
    pub unindexed: BTreeMap<String, String>,
    /// Why the repository files could not be listed, when they could not.
    pub unlisted: Option<String>,
}

/// Index each tier-1 text with `blocks_of`, in path order, until `in_budget` answers false.
///
/// # Errors
///
/// Returns an error when a path that the budget reaches is not tier-1 source, or when a block
/// source fails in a way that is not a parse or tokenizer failure.
pub fn clone_index<B, T>(
    texts: &BTreeMap<String, String>,
    unread: &BTreeMap<String, String>,
    mut blocks_of: B,
    mut in_budget: T,
) -> Result<IndexedFiles, Error>
where
    B: FnMut(&str, &str, Language) -> Result<Vec<Block>, SideError>,
    T: FnMut() -> bool,
{
    let mut built = IndexedFiles {
        unindexed: unread.clone(),
        ..IndexedFiles::default()
    };
    let mut stopped = false;
    for (path, text) in texts {
        stopped = stopped || !in_budget();
        if stopped {
            let reason = messages::CLONE_INDEX_STOPPED.to_owned();
            built.unindexed.insert(path.clone(), reason);
            continue;
        }
        let language =
            pair_language(path).ok_or_else(|| Error::NotIndexable { path: path.clone() })?;
        match blocks_of(path, text, language) {
            Ok(blocks) => {
                built.files.insert(path.clone(), blocks);
            }
            Err(error) => {
                let reason = side_reason(path, &error, &CLONE_REASONS)?;
                built.unindexed.insert(path.clone(), reason);
            }
        }
    }
    Ok(built)
}
