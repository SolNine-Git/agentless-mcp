//! The index operation: walk and read the repository, plan each file, parse and encode the
//! misses on a bounded pool, write agentless.db in one transaction, then sweep the cache root.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use store::{
    Analysis, Extract, FileText, NewMeta, Observed, Read, RowCounts, Settled, WriteError, Writer,
};
pub use store::{EvictionReport, IndexFailure, IndexReport, IndexSkip};
use syntax::{Language, Unparsable};

use crate::parallel::{map_in_order, threads};
use crate::repo::root_text;
use crate::tags::repo_generation;
use crate::{Error, Noted, Repo, Services, in_metric_scope};

/// What one index run is asked to do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexRequest {
    /// Extract every file, even one whose row still describes it.
    pub force: bool,
    /// The start-up value of `AGENTLESS_MCP_MAX_CACHE_BYTES`, which sets the sweep's ceiling.
    pub max_cache_bytes: Option<OsString>,
}

/// Builds or refreshes the agentless.db of `repo` at the generation of `snapshot`: the walk and
/// the reads, the plan, one parse of each file that changed, one write, then the ceiling sweep.
/// The warnings come in the order they arose.
#[must_use]
pub fn index(
    services: &Services,
    repo: &Repo,
    snapshot: &git::Snapshot,
    request: &IndexRequest,
) -> Noted<Result<IndexReport, Error>> {
    let parse = |planned: &Extract| store::analyze(&planned.file, planned.measure);
    let parser = Parser {
        threads: threads(),
        parse: &parse,
    };
    index_with(services, repo, snapshot, request, &parser)
}

// The pool size and the parse of one planned file: the tests swap both.
pub(crate) struct Parser<'a> {
    pub(crate) threads: usize,
    pub(crate) parse: &'a (dyn Fn(&Extract) -> Result<Analysis, Unparsable> + Sync),
}

pub(crate) fn index_with(
    services: &Services,
    repo: &Repo,
    snapshot: &git::Snapshot,
    request: &IndexRequest,
    parser: &Parser<'_>,
) -> Noted<Result<IndexReport, Error>> {
    let place = match Place::new(services, &repo.root) {
        Ok(place) => place,
        Err(error) => return Noted::new(Err(error)),
    };
    let tree = snapshot.tree_oid.as_deref();
    let written = written(services, &place, tree, request.force, parser);
    let mut diagnostics = place.warnings.clone();
    diagnostics.extend(written.diagnostics);
    let report = written.value.map(|written| {
        let (evicted, said) = swept(services, &place, request);
        diagnostics.extend(said);
        let Written {
            settled,
            counts,
            generation,
        } = written;
        settled.report(place.database, generation, counts, evicted)
    });
    Noted {
        value: report,
        diagnostics,
    }
}

// Where the repository's database lies, and the warning that the cache root gave.
struct Place {
    cache_root: PathBuf,
    root: PathBuf,
    repo_root: String,
    database: PathBuf,
    warnings: Vec<String>,
}

impl Place {
    fn new(services: &Services, root: &Path) -> Result<Place, Error> {
        let cache_root = services
            .cache_root
            .as_ref()
            .map_err(|home| kernel::Error::OperationFailed(home.to_string()))?;
        let repo_root = root_text(root)?.to_owned();
        Ok(Place {
            database: store::facts_database_path(&cache_root.path, &repo_root),
            cache_root: cache_root.path.clone(),
            root: root.to_path_buf(),
            repo_root,
            warnings: cache_root.ignored.iter().map(ToString::to_string).collect(),
        })
    }
}

struct Written {
    settled: Settled,
    counts: RowCounts,
    generation: String,
}

// Everything under the lock: the open, the walk, the plan, the parses, the generation, the write.
fn written(
    services: &Services,
    place: &Place,
    tree: Option<&str>,
    force: bool,
    parser: &Parser<'_>,
) -> Noted<Result<Written, Error>> {
    let (opened, mut diagnostics) = Writer::open(&place.database);
    let value = opened
        .map_err(|error| refused(place, error))
        .and_then(|writer| {
            let walk = services.git.walk(&place.root);
            diagnostics.extend(walk.warnings.iter().map(ToString::to_string));
            let files = walk
                .files
                .map_err(|refusal| Error::from(kernel::Error::from(refusal)))?;
            let plan = store::plan(
                writer.previous(),
                observed(&place.root, files),
                store::analysis(),
                force,
            );
            // Each worker encodes the blobs of its file, so the write runs only the inserts.
            let parsed = map_in_order(&plan.extract, parser.threads, |planned| {
                (parser.parse)(planned).map(|analysis| store::encode(&analysis))
            });
            let settled = store::settle(plan, parsed);
            let generation = repo_generation(services, &place.root, tree);
            diagnostics.extend(generation.diagnostics);
            let generation = generation.value?;
            let meta = NewMeta {
                repo_root: &place.repo_root,
                generation: &generation,
                created: seconds(services.clock.wall()),
            };
            let counts = writer
                .write(&settled, meta)
                .map_err(|error| unbuilt(place, &error))?;
            Ok(Written {
                settled,
                counts,
                generation,
            })
        });
    Noted { value, diagnostics }
}

// The walked files of a known language, each with its read.
fn observed(root: &Path, files: Vec<git::RepoFile>) -> Vec<Observed> {
    files
        .into_iter()
        .filter_map(|file| {
            let language = Language::from_path(&file.path)?;
            Some(observe(root, file, language))
        })
        .collect()
}

// A file over the cap is decided from the walk's size, so its bytes are never read.
fn observe(root: &Path, file: git::RepoFile, language: Language) -> Observed {
    let measure = in_metric_scope(&file.path) && metrics::stamp(language).is_some();
    let cap = u64::try_from(io::MAX_FILE_BYTES).unwrap_or(u64::MAX);
    let read = if file.size > cap {
        Read::Oversize { size: file.size }
    } else {
        match io::read_bounded(&root.join(&file.path)) {
            Ok(bytes) => Read::Text(FileText::decode(file.path.clone(), language, &bytes)),
            Err(skipped) => Read::Unreadable {
                reason: skipped.to_string(),
            },
        }
    };
    Observed {
        path: file.path,
        language,
        measure,
        read,
    }
}

// After the lock is released: the sweep takes the lock of each database that it deletes.
fn swept(
    services: &Services,
    place: &Place,
    request: &IndexRequest,
) -> (EvictionReport, Vec<String>) {
    let (limit, warning) = store::max_cache_bytes(request.max_cache_bytes.as_deref());
    let now = services
        .clock
        .wall()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    let (evicted, said) =
        store::enforce_cache_limit(&place.cache_root, &place.database, limit, now);
    (evicted, warning.into_iter().chain(said).collect())
}

fn refused(place: &Place, error: WriteError) -> Error {
    match error {
        WriteError::Locked { directory } => {
            let message = format!(
                "could not take the index lock for {}: another index run may hold it, or {} may \
                 be on a filesystem that does not support locking",
                place.repo_root,
                directory.display()
            );
            kernel::Error::CacheLocked(message).into()
        }
        WriteError::Io(error) => unbuilt(place, &io::strerror(&error)),
        WriteError::Sql(error) => unbuilt(place, &error),
        WriteError::Refused(refusal) => unbuilt(place, &refusal),
    }
}

fn unbuilt(place: &Place, reason: &dyn std::fmt::Display) -> Error {
    let message = format!(
        "cannot build the tag cache for {}: {reason}",
        place.repo_root
    );
    kernel::Error::OperationFailed(message).into()
}

// Whole seconds since the epoch; a clock before the epoch gives zero.
fn seconds(time: SystemTime) -> i64 {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests;
