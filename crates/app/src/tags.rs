//! The tag cache an operation reads: opened read-only, a refused database discarded, and the
//! status that the receipt and the capabilities report describe.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use store::{CacheState, CacheStatus, ManifestEntry, RowCounts, SqlError, TagsDb};
use syntax::Language;

use crate::pairs::Stored;
use crate::{Error, Noted, Repo, Resolved, Services};

/// The tag cache of one repository, as lint reads stored clone blocks from it.
#[derive(Debug)]
pub struct Tags {
    db: Option<TagsDb>,
}

impl Tags {
    /// Opens the tag cache of `repo` for the hook form: the open, the discard of a refused
    /// database, and the time stamp of a usable one. It reads no generation and no row counts.
    ///
    /// # Errors
    ///
    /// Returns an operation failure when the home directory that the cache root needs is unknown.
    pub fn open(services: &Services, repo: &Repo) -> Result<Noted<Tags>, Error> {
        let opened = Opened::new(services, &repo.root, false)?;
        Ok(Noted {
            value: Tags::stamped(opened.source, &opened.database),
            diagnostics: opened.diagnostics,
        })
    }

    // Only a usable database gets the time stamp, which keeps it out of the eviction sweep.
    fn stamped(source: store::Source, database: &Path) -> Tags {
        let db = match source {
            store::Source::Usable(db) => {
                touch(database);
                Some(db)
            }
            _ => None,
        };
        Tags { db }
    }

    /// Returns true when a usable tag cache backs the clone index.
    #[must_use]
    pub fn cached(&self) -> bool {
        self.db.is_some()
    }

    pub(crate) fn stored(&self, path: &str, text: &str, language: Language) -> Stored {
        let Some(db) = &self.db else {
            return Stored::Miss;
        };
        match db.blocks_for(path, text, language) {
            store::Lookup::Hit(blocks) => Stored::Hit(blocks),
            store::Lookup::Unmeasured(reason) => Stored::Unmeasured(reason),
            store::Lookup::Miss => Stored::Miss,
            store::Lookup::Unreadable(failure) => Stored::Unreadable(failure.to_string()),
        }
    }
}

/// What the receipt and the capabilities report say about the tag cache of one call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheReport {
    /// The database path and what the call found there.
    pub status: CacheStatus,
    /// The row counts of a usable database, zero without one, or the error of a count.
    pub counts: Result<RowCounts, SqlError>,
    /// The `cache:` field of the receipt.
    pub receipt: String,
}

/// One command's repository, its git state, and its tag cache.
#[derive(Debug)]
pub struct Session {
    /// The repository.
    pub repo: Repo,
    /// HEAD, the tree, the dirty count, and the note on what is unknown.
    pub snapshot: git::Snapshot,
    /// The tag cache that lint reads blocks from.
    pub tags: Tags,
    /// The cache status, its row counts and its receipt text.
    pub cache: CacheReport,
}

impl Session {
    /// Opens the tag cache of `resolved`, unless `no_cache` asks to parse every file: the open,
    /// the discard of a refused database, the generation, the time stamp, and the row counts.
    ///
    /// # Errors
    ///
    /// Returns an operation failure when the cache root's home directory is unknown, and the
    /// walk refusal when the generation of a tree outside git needs a walk that fails.
    pub fn open(
        services: &Services,
        resolved: Resolved,
        no_cache: bool,
    ) -> Result<Noted<Session>, Error> {
        let Resolved { repo, snapshot } = resolved;
        let opened = Opened::new(services, &repo.root, no_cache)?;
        let tree = snapshot.tree_oid.as_deref();
        let (state, tags) = settled(services, &repo.root, tree, opened.source, &opened.database)?;
        let status = CacheStatus {
            path: opened.database,
            state,
        };
        let cache = report(status, tags.db.as_ref());
        let session = Session {
            repo,
            snapshot,
            tags,
            cache,
        };
        Ok(Noted {
            value: session,
            diagnostics: opened.diagnostics,
        })
    }
}

// The state the receipt describes. A usable database gets its generation first, then the stamp.
fn settled(
    services: &Services,
    root: &Path,
    tree: Option<&str>,
    source: store::Source,
    database: &Path,
) -> Result<(CacheState, Tags), Error> {
    let state = match &source {
        store::Source::Bypassed => CacheState::Bypassed,
        store::Source::Absent => CacheState::Absent { note: None },
        store::Source::Refused(refusal) => CacheState::Absent {
            note: Some(refusal.clone()),
        },
        store::Source::Usable(db) => indexed(services, root, tree, db)?,
    };
    Ok((state, Tags::stamped(source, database)))
}

// The open of one repository's database, and the warnings that it and a discard gave.
struct Opened {
    database: PathBuf,
    source: store::Source,
    diagnostics: Vec<String>,
}

impl Opened {
    fn new(services: &Services, root: &Path, no_cache: bool) -> Result<Opened, Error> {
        let cache_root = services
            .cache_root
            .as_ref()
            .map_err(|home| kernel::Error::OperationFailed(home.to_string()))?;
        let mut diagnostics: Vec<String> =
            cache_root.ignored.iter().map(ToString::to_string).collect();
        let repo_root = utf8(root)?;
        let database = store::database_path(&cache_root.path, repo_root);
        let source = store::open_source(&database, repo_root, no_cache);
        if let store::Source::Refused(refusal) = &source
            && refusal.discards()
            && let store::Discarded::Failed(failure) = store::discard(&database)
        {
            diagnostics.push(failure.to_string());
        }
        Ok(Opened {
            database,
            source,
            diagnostics,
        })
    }
}

// The generation of the repository now: the tree id, or a digest of the walked files.
fn indexed(
    services: &Services,
    root: &Path,
    tree: Option<&str>,
    db: &TagsDb,
) -> Result<CacheState, Error> {
    let repo_generation = match tree {
        Some(tree) => tree.to_owned(),
        None => store::nogit_generation(&manifest(services, root)?),
    };
    Ok(CacheState::Indexed {
        generation: db.generation().to_owned(),
        repo_generation,
        repo_root: utf8(root)?.to_owned(),
    })
}

fn manifest(services: &Services, root: &Path) -> Result<Vec<ManifestEntry>, Error> {
    let walk = services.git.walk(root).map_err(kernel::Error::from)?;
    let entries = walk
        .files
        .into_iter()
        .map(|file| {
            let stamp = root
                .join(&file.path)
                .metadata()
                .ok()
                .map(|meta| store::FileStamp {
                    size: meta.size(),
                    mtime_ns: i128::from(meta.mtime()) * 1_000_000_000
                        + i128::from(meta.mtime_nsec()),
                });
            ManifestEntry {
                path: file.path,
                stamp,
            }
        })
        .collect();
    Ok(entries)
}

// A failed stamp only ages the database for the eviction sweep, so it prints nothing.
fn touch(database: &Path) {
    let _ = io::touch(database);
}

fn report(status: CacheStatus, db: Option<&TagsDb>) -> CacheReport {
    let none = RowCounts {
        files: 0,
        tags: 0,
        imports: 0,
        refs: 0,
    };
    let counts = db.map_or(Ok(none), TagsDb::row_counts);
    let receipt = match &counts {
        Ok(_) => status.receipt(),
        Err(error) => format!("none (cache status unavailable: {error})"),
    };
    CacheReport {
        status,
        counts,
        receipt,
    }
}

fn utf8(root: &Path) -> Result<&str, Error> {
    root.to_str().ok_or_else(|| {
        let message = format!("not a UTF-8 path: {}", root.display());
        kernel::Error::RepoResolution(message).into()
    })
}

#[cfg(test)]
mod tests;
