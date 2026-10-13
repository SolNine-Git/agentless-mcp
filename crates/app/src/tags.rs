//! The cache an operation reads: the tag cache for lint and the hook, agentless.db for the read
//! operations and capabilities. Each opens read-only, a refused database is discarded, and the
//! status describes it for the receipt and the capabilities report.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::thread;

use store::{
    CacheState, CacheStatus, FactSource, FactsDb, ManifestEntry, RowCounts, SqlError, TagsDb,
};
use syntax::Language;

use crate::pairs::Stored;
use crate::repo::root_text;
use crate::{Error, Noted, Repo, Services};

const NO_ROWS: RowCounts = RowCounts {
    files: 0,
    tags: 0,
    imports: 0,
    refs: 0,
};

/// The tag cache of one repository, as lint reads stored clone blocks from it.
#[derive(Debug)]
pub struct Tags {
    db: Option<TagsDb>,
}

impl Tags {
    /// Opens the tag cache of `repo` for the hook form: the open, the discard of a refused
    /// database, and the time stamp of a usable one. It reads no generation and no row counts.
    /// The error is an operation failure when the home directory that the cache root needs is
    /// unknown.
    #[must_use]
    pub fn open(services: &Services, repo: &Repo) -> Noted<Result<Tags, Error>> {
        let located = match Located::new::<TagsDb>(services, &repo.root) {
            Ok(located) => located,
            Err(error) => return Noted::new(Err(error)),
        };
        let Opened {
            database,
            source,
            mut diagnostics,
        } = located.open::<TagsDb>(false);
        diagnostics.extend(discarded(&source, &database));
        Noted {
            value: Ok(Tags {
                db: stamped(source, &database),
            }),
            diagnostics,
        }
    }

    // True when a usable tag cache backs the clone index.
    pub(crate) fn cached(&self) -> bool {
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

// Only a usable database gets the time stamp, which keeps it out of the eviction sweep.
fn stamped<D>(source: store::Source<D>, database: &Path) -> Option<D> {
    match source {
        store::Source::Usable(db) => {
            touch(database);
            Some(db)
        }
        _ => None,
    }
}

// A failed stamp only ages the database for the eviction sweep, so it prints nothing.
fn touch(database: &Path) {
    let _ = io::touch(database);
}

/// What the receipt and the capabilities report say about the cache of one call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheReport {
    /// The database path and what the call found there.
    pub status: CacheStatus,
    /// The row counts of a usable database, zero without one, or the error of a count.
    pub counts: Result<RowCounts, SqlError>,
    /// The `cache:` field of the receipt.
    pub receipt: String,
}

/// One command's repository, its git state, and the cache it reads.
#[derive(Debug)]
pub struct Session {
    /// The repository.
    pub repo: Repo,
    /// HEAD, the tree, the dirty count, and the note on what is unknown.
    pub snapshot: git::Snapshot,
    /// The tag cache that lint reads blocks from; closed in a session over agentless.db.
    pub tags: Tags,
    /// The facts and metrics of each file: agentless.db rows in a session over agentless.db,
    /// else parses.
    pub facts: FactSource,
    /// The cache status, its row counts and its receipt text.
    pub cache: CacheReport,
}

/// The snapshot note, which comes first, and the session or the error that stopped its open.
#[derive(Debug)]
pub struct Opening {
    /// Each reason a git value is unknown or borrowed, joined with "; ", or empty.
    pub note: String,
    /// The session, or an operation failure when the cache root's home directory is unknown, or
    /// the walk refusal when the generation of a tree outside git needs a walk that fails. The
    /// warnings come with either outcome.
    pub session: Noted<Result<Session, Error>>,
}

impl Session {
    /// Reads the git state of `repo` and opens its tag cache, unless `no_cache` asks to parse every
    /// file: the open, the discard of a refused database, the generation, the time stamp, and the
    /// row counts. Lint reads this session.
    #[must_use]
    pub fn open(services: &Services, repo: Repo, no_cache: bool) -> Opening {
        open_with::<TagsDb>(services, repo, no_cache)
    }

    /// Reads the git state of `repo` and opens its agentless.db in the same steps as
    /// [`Session::open`]. The read operations and capabilities read this session.
    #[must_use]
    pub fn open_facts(services: &Services, repo: Repo, no_cache: bool) -> Opening {
        open_with::<FactsDb>(services, repo, no_cache)
    }
}

fn open_with<D: CacheFile>(services: &Services, repo: Repo, no_cache: bool) -> Opening {
    let enclosing = services.git.root(&repo.root);
    let (snapshot, counted) =
        read_and_open::<D>(services, &repo.root, enclosing.as_deref(), no_cache);
    let note = snapshot.note.clone();
    let session = match counted {
        Ok(counted) => assembled(services, repo, snapshot, counted),
        Err(error) => Noted::new(Err(error)),
    };
    Opening { note, session }
}

// The cache file that a session opens, and the two readers that a session holds of it.
trait CacheFile: Sized {
    fn path(cache_root: &Path, repo_root: &str) -> PathBuf;
    fn open(database: &Path, repo_root: &str, no_cache: bool) -> store::Source<Self>;
    fn discard(database: &Path) -> store::Discarded;
    fn generation(&self) -> &str;
    fn row_counts(&self) -> Result<RowCounts, SqlError>;
    fn readers(db: Option<Self>) -> (Tags, FactSource);
}

impl CacheFile for TagsDb {
    fn path(cache_root: &Path, repo_root: &str) -> PathBuf {
        store::database_path(cache_root, repo_root)
    }

    fn open(database: &Path, repo_root: &str, no_cache: bool) -> store::Source<Self> {
        store::open_source(database, repo_root, no_cache)
    }

    fn discard(database: &Path) -> store::Discarded {
        store::discard(database)
    }

    fn generation(&self) -> &str {
        TagsDb::generation(self)
    }

    fn row_counts(&self) -> Result<RowCounts, SqlError> {
        TagsDb::row_counts(self)
    }

    fn readers(db: Option<Self>) -> (Tags, FactSource) {
        (Tags { db }, FactSource::parsing())
    }
}

impl CacheFile for FactsDb {
    fn path(cache_root: &Path, repo_root: &str) -> PathBuf {
        store::facts_database_path(cache_root, repo_root)
    }

    fn open(database: &Path, repo_root: &str, no_cache: bool) -> store::Source<Self> {
        store::open_facts(database, repo_root, no_cache)
    }

    fn discard(database: &Path) -> store::Discarded {
        store::discard_facts(database)
    }

    fn generation(&self) -> &str {
        FactsDb::generation(self)
    }

    fn row_counts(&self) -> Result<RowCounts, SqlError> {
        FactsDb::row_counts(self)
    }

    fn readers(db: Option<Self>) -> (Tags, FactSource) {
        let facts = db.map_or_else(FactSource::parsing, FactSource::reading);
        (Tags { db: None }, facts)
    }
}

// A read-only open creates files beside the database, and `git status` counts them when they lie
// in the worktree. So only a database outside the worktree opens while the snapshot runs.
fn read_and_open<D: CacheFile>(
    services: &Services,
    root: &Path,
    enclosing: Option<&Path>,
    no_cache: bool,
) -> (git::Snapshot, Result<Counted<D>, Error>) {
    let snapshot = || services.git.snapshot(root, enclosing);
    match Located::new::<D>(services, root) {
        Ok(located) if outside(&located.database, enclosing) => thread::scope(|scope| {
            let running = io::spawn_or_run(scope, &snapshot);
            let counted = located.open::<D>(no_cache).counted();
            (running.joined(), Ok(counted))
        }),
        located => {
            let snapshot = snapshot();
            let counted = located.map(|located| located.open::<D>(no_cache).counted());
            (snapshot, counted)
        }
    }
}

fn outside(database: &Path, enclosing: Option<&Path>) -> bool {
    enclosing
        .is_some_and(|top| io::resolve(database).is_ok_and(|resolved| !resolved.starts_with(top)))
}

// The discard, the generation and the stamp come after the snapshot. The warnings keep that
// order: the cache root, the discard, then the generation walk.
fn assembled<D: CacheFile>(
    services: &Services,
    repo: Repo,
    snapshot: git::Snapshot,
    counted: Counted<D>,
) -> Noted<Result<Session, Error>> {
    let Counted { opened, counts } = counted;
    let Opened {
        database,
        source,
        mut diagnostics,
    } = opened;
    diagnostics.extend(discarded(&source, &database));
    let tree = snapshot.tree_oid.as_deref();
    let settled = settled(services, &repo.root, tree, source, &database);
    diagnostics.extend(settled.diagnostics);
    let session = settled.value.map(|(state, db)| {
        let (tags, facts) = D::readers(db);
        let status = CacheStatus {
            path: database,
            state,
        };
        Session {
            repo,
            snapshot,
            tags,
            facts,
            cache: report(status, counts),
        }
    });
    Noted {
        value: session,
        diagnostics,
    }
}

// The state the receipt describes. A usable database gets its generation first, then the stamp.
fn settled<D: CacheFile>(
    services: &Services,
    root: &Path,
    tree: Option<&str>,
    source: store::Source<D>,
    database: &Path,
) -> Noted<Result<(CacheState, Option<D>), Error>> {
    let state = match &source {
        store::Source::Bypassed => Noted::new(Ok(CacheState::Bypassed)),
        store::Source::Absent => Noted::new(Ok(CacheState::Absent { note: None })),
        store::Source::Refused(refusal) => Noted::new(Ok(CacheState::Absent {
            note: Some(refusal.clone()),
        })),
        store::Source::Usable(db) => indexed(services, root, tree, db.generation()),
    };
    state.map(|state| state.map(|state| (state, stamped(source, database))))
}

// Where one repository's database lies, and the warnings that its cache root gave.
struct Located {
    database: PathBuf,
    repo_root: String,
    diagnostics: Vec<String>,
}

impl Located {
    fn new<D: CacheFile>(services: &Services, root: &Path) -> Result<Located, Error> {
        let cache_root = services
            .cache_root
            .as_ref()
            .map_err(|home| kernel::Error::OperationFailed(home.to_string()))?;
        let diagnostics = cache_root.ignored.iter().map(ToString::to_string).collect();
        let repo_root = root_text(root)?.to_owned();
        let database = D::path(&cache_root.path, &repo_root);
        Ok(Located {
            database,
            repo_root,
            diagnostics,
        })
    }

    fn open<D: CacheFile>(self, no_cache: bool) -> Opened<D> {
        let source = D::open(&self.database, &self.repo_root, no_cache);
        Opened {
            database: self.database,
            source,
            diagnostics: self.diagnostics,
        }
    }
}

// The open of one repository's database, and the warnings that its cache root gave.
struct Opened<D> {
    database: PathBuf,
    source: store::Source<D>,
    diagnostics: Vec<String>,
}

impl<D: CacheFile> Opened<D> {
    fn counted(self) -> Counted<D> {
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
struct Counted<D> {
    opened: Opened<D>,
    counts: Result<RowCounts, SqlError>,
}

// Removes a refused database that the store says to discard, and returns a failed removal.
fn discarded<D: CacheFile>(source: &store::Source<D>, database: &Path) -> Option<String> {
    if let store::Source::Refused(refusal) = source
        && refusal.discards()
        && let store::Discarded::Failed(failure) = D::discard(database)
    {
        return Some(failure.to_string());
    }
    None
}

// The generation that the database was built at, beside the repository's generation now.
fn indexed(
    services: &Services,
    root: &Path,
    tree: Option<&str>,
    built: &str,
) -> Noted<Result<CacheState, Error>> {
    repo_generation(services, root, tree).map(|generation| {
        Ok(CacheState::Indexed {
            generation: built.to_owned(),
            repo_generation: generation?,
            repo_root: root_text(root)?.to_owned(),
        })
    })
}

// The generation of the repository now: the tree id, or a digest of the walked files.
pub(crate) fn repo_generation(
    services: &Services,
    root: &Path,
    tree: Option<&str>,
) -> Noted<Result<String, Error>> {
    match tree {
        Some(tree) => Noted::new(Ok(tree.to_owned())),
        None => {
            manifest(services, root).map(|entries| entries.map(|e| store::nogit_generation(&e)))
        }
    }
}

// The walk warnings come with either outcome, as the walk raised them.
fn manifest(services: &Services, root: &Path) -> Noted<Result<Vec<ManifestEntry>, Error>> {
    let walk = services.git.walk(root);
    let entries = walk
        .files
        .map_err(|refusal| kernel::Error::from(refusal).into());
    Noted {
        value: entries.map(|files| files.into_iter().map(|file| entry(root, file)).collect()),
        diagnostics: walk.warnings.iter().map(ToString::to_string).collect(),
    }
}

fn entry(root: &Path, file: git::RepoFile) -> ManifestEntry {
    let stamp = root
        .join(&file.path)
        .metadata()
        .ok()
        .map(|meta| store::FileStamp {
            size: meta.size(),
            mtime_ns: i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()),
        });
    ManifestEntry {
        path: file.path,
        stamp,
    }
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

#[cfg(test)]
mod tests;
