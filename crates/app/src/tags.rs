//! The tag cache an operation reads: opened read-only, a refused database discarded, and the
//! status that the receipt and the capabilities report describe.

use std::os::unix::fs::MetadataExt;
use std::panic;
use std::path::{Path, PathBuf};
use std::thread;

use store::{CacheState, CacheStatus, ManifestEntry, RowCounts, SqlError, TagsDb};
use syntax::Language;

use crate::pairs::Stored;
use crate::{Error, Noted, Repo, Services};

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
        let Opened {
            database,
            source,
            mut diagnostics,
        } = Located::new(services, &repo.root)?.open(false);
        diagnostics.extend(discarded(&source, &database));
        Ok(Noted {
            value: Tags::stamped(source, &database),
            diagnostics,
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

/// The snapshot note, which comes first, and the session or the error that stopped its open.
#[derive(Debug)]
pub struct Opening {
    /// Each reason a git value is unknown or borrowed, joined with "; ", or empty.
    pub note: String,
    /// The session, or an operation failure when the cache root's home directory is unknown, or
    /// the walk refusal when the generation of a tree outside git needs a walk that fails.
    pub session: Result<Noted<Session>, Error>,
}

impl Session {
    /// Reads the git state of `repo` and opens its tag cache, unless `no_cache` asks to parse every
    /// file: the open, the discard of a refused database, the generation, the time stamp, and the
    /// row counts.
    #[must_use]
    pub fn open(services: &Services, repo: Repo, no_cache: bool) -> Opening {
        let enclosing = services.git.root(&repo.root);
        let (snapshot, counted) =
            read_and_open(services, &repo.root, enclosing.as_deref(), no_cache);
        let note = snapshot.note.clone();
        let session = counted.and_then(|counted| assembled(services, repo, snapshot, counted));
        Opening { note, session }
    }
}

// A read-only open creates files beside the database, and `git status` counts them when they lie
// in the worktree. So only a database outside the worktree opens while the snapshot runs.
fn read_and_open(
    services: &Services,
    root: &Path,
    enclosing: Option<&Path>,
    no_cache: bool,
) -> (git::Snapshot, Result<Counted, Error>) {
    let snapshot = || services.git.snapshot(root, enclosing);
    match Located::new(services, root) {
        Ok(located) if outside(&located.database, enclosing) => thread::scope(|scope| {
            let running = scope.spawn(snapshot);
            let counted = located.open(no_cache).counted();
            let snapshot = running
                .join()
                .unwrap_or_else(|payload| panic::resume_unwind(payload));
            (snapshot, Ok(counted))
        }),
        located => {
            let snapshot = snapshot();
            let counted = located.map(|located| located.open(no_cache).counted());
            (snapshot, counted)
        }
    }
}

fn outside(database: &Path, enclosing: Option<&Path>) -> bool {
    enclosing
        .is_some_and(|top| io::resolve(database).is_ok_and(|resolved| !resolved.starts_with(top)))
}

// The discard, the generation and the stamp come after the snapshot.
fn assembled(
    services: &Services,
    repo: Repo,
    snapshot: git::Snapshot,
    counted: Counted,
) -> Result<Noted<Session>, Error> {
    let Counted { opened, counts } = counted;
    let Opened {
        database,
        source,
        mut diagnostics,
    } = opened;
    diagnostics.extend(discarded(&source, &database));
    let tree = snapshot.tree_oid.as_deref();
    let (state, tags) = settled(services, &repo.root, tree, source, &database)?;
    let status = CacheStatus {
        path: database,
        state,
    };
    let session = Session {
        repo,
        snapshot,
        tags,
        cache: report(status, counts),
    };
    Ok(Noted {
        value: session,
        diagnostics,
    })
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

// Where one repository's database lies, and the warnings that its cache root gave.
struct Located {
    database: PathBuf,
    repo_root: String,
    diagnostics: Vec<String>,
}

impl Located {
    fn new(services: &Services, root: &Path) -> Result<Located, Error> {
        let cache_root = services
            .cache_root
            .as_ref()
            .map_err(|home| kernel::Error::OperationFailed(home.to_string()))?;
        let diagnostics = cache_root.ignored.iter().map(ToString::to_string).collect();
        let repo_root = utf8(root)?.to_owned();
        let database = store::database_path(&cache_root.path, &repo_root);
        Ok(Located {
            database,
            repo_root,
            diagnostics,
        })
    }

    fn open(self, no_cache: bool) -> Opened {
        let source = store::open_source(&self.database, &self.repo_root, no_cache);
        Opened {
            database: self.database,
            source,
            diagnostics: self.diagnostics,
        }
    }
}

// The open of one repository's database, and the warnings that its cache root gave.
struct Opened {
    database: PathBuf,
    source: store::Source,
    diagnostics: Vec<String>,
}

impl Opened {
    fn counted(self) -> Counted {
        let counts = match &self.source {
            store::Source::Usable(db) => db.row_counts(),
            _ => Ok(NO_ROWS),
        };
        Counted {
            opened: self,
            counts,
        }
    }
}

// An open database and its row counts: zero without a usable database, or the error of a count.
struct Counted {
    opened: Opened,
    counts: Result<RowCounts, SqlError>,
}

const NO_ROWS: RowCounts = RowCounts {
    files: 0,
    tags: 0,
    imports: 0,
    refs: 0,
};

// Removes a refused database that the store says to discard, and returns a failed removal.
fn discarded(source: &store::Source, database: &Path) -> Option<String> {
    if let store::Source::Refused(refusal) = source
        && refusal.discards()
        && let store::Discarded::Failed(failure) = store::discard(database)
    {
        return Some(failure.to_string());
    }
    None
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

fn report(status: CacheStatus, counts: Result<RowCounts, SqlError>) -> CacheReport {
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
