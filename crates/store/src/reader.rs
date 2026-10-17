//! The read side of agentless.db: the open with its admission checks, the freshness test of one
//! file's row, and the fetch of its blobs, all in one read snapshot.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::Connection;

use crate::freshness::{analysis_rejection, facts_schema_rejection, rejection};
use crate::open::{BUSY_TIMEOUT, connect, fetch, found, has_meta, read_meta};
use crate::value::Cell;
use crate::{
    FACTS_SCHEMA_VERSION, FileText, IndexMeta, Refusal, RowCounts, Source, SqlClass, SqlError,
    schema,
};

/// An open agentless.db that passed the admission checks, with one read snapshot. The drop ends
/// the snapshot. One mutex serializes its reads, so threads can share it.
#[derive(Debug)]
pub struct FactsDb {
    connection: Mutex<Connection>,
    database: PathBuf,
    generation: String,
}

/// Opens agentless.db at `database` for the repository `repo_root`, unless `no_cache` asks to
/// parse every file instead. The database is opened read-only, and a refused one is not changed.
#[must_use]
pub fn open_facts(database: &Path, repo_root: &str, no_cache: bool) -> Source<FactsDb> {
    if no_cache {
        return Source::Bypassed;
    }
    open_facts_within(database, repo_root, BUSY_TIMEOUT)
}

pub(crate) fn open_facts_within(
    database: &Path,
    repo_root: &str,
    busy: Duration,
) -> Source<FactsDb> {
    found(database, || FactsDb::open(database, repo_root, busy))
}

impl FactsDb {
    fn open(database: &Path, repo_root: &str, busy: Duration) -> Result<Self, Refusal> {
        let connection = connect(database, busy)?;
        // One deferred read transaction pins every later read to the same snapshot.
        connection
            .execute("BEGIN", ())
            .map_err(SqlError::from_sqlite)?;
        // The schema is read alone first: another layout may not hold the other meta columns.
        let found = stored_schema(&connection)?.ok_or(Refusal::NoIndex)?;
        if let Some(refusal) = facts_schema_rejection(found) {
            return Err(refusal);
        }
        let (meta, stored) = read_meta(&connection, schema::META)?.ok_or(Refusal::NoIndex)?;
        let analysis = match stored {
            Cell::Blob(bytes) => Some(bytes),
            _ => None,
        };
        let meta = IndexMeta { analysis, ..meta };
        let refused =
            rejection(&meta, FACTS_SCHEMA_VERSION, repo_root).or_else(|| analysis_rejection(&meta));
        if let Some(refusal) = refused {
            return Err(refusal);
        }
        Ok(Self {
            connection: Mutex::new(connection),
            database: database.to_path_buf(),
            generation: meta.generation,
        })
    }

    /// Returns the generation that the index was built at: the git tree id, or a `nogit:`
    /// manifest digest.
    #[must_use]
    pub fn generation(&self) -> &str {
        &self.generation
    }

    /// Counts the indexed files and the symbols, imports and references that they hold.
    ///
    /// # Errors
    ///
    /// Returns the [`SqlError`] of a count that fails.
    pub fn row_counts(&self) -> Result<RowCounts, SqlError> {
        row_counts(&self.connection())
    }

    pub(crate) fn path(&self) -> &Path {
        &self.database
    }

    // The id of the row that describes `file` now: for this language, from exactly this text. The
    // open admitted only rows of this analysis.
    pub(crate) fn fresh_id(&self, file: &FileText) -> Result<Option<i64>, SqlError> {
        let found = fetch::<3>(&self.connection(), schema::LOOKUP, [&file.path], 1)?;
        let Some([id, language, digest]) = found.into_iter().next() else {
            return Ok(None);
        };
        let fresh = language.equals(Some(file.language.name()))
            && digest == Cell::Blob(file.digest().bytes().to_vec());
        if !fresh {
            return Ok(None);
        }
        id.int().map(Some)
    }

    // The cells that `sql` selects from the row `id`.
    pub(crate) fn cells<const N: usize>(&self, sql: &str, id: i64) -> Result<[Cell; N], SqlError> {
        let found = fetch::<N>(&self.connection(), sql, [id], 1)?;
        found
            .into_iter()
            .next()
            .ok_or_else(|| SqlError::new(SqlClass::Interface, format!("no files row with id {id}")))
    }

    // A panic while the lock was held leaves the connection as usable as before.
    fn connection(&self) -> MutexGuard<'_, Connection> {
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

// The schema version of the meta row, or None with no meta table or no meta row.
pub(crate) fn stored_schema(connection: &Connection) -> Result<Option<i64>, SqlError> {
    if !has_meta(connection)? {
        return Ok(None);
    }
    let found = fetch::<1>(connection, schema::SCHEMA, (), 1)?;
    found
        .into_iter()
        .next()
        .map(|[schema]| schema.int())
        .transpose()
}

pub(crate) fn row_counts(connection: &Connection) -> Result<RowCounts, SqlError> {
    let found = fetch::<4>(connection, schema::COUNTS, (), 1)?;
    let Some([files, tags, imports, refs]) = found.into_iter().next() else {
        let message = "the count query gave no row";
        return Err(SqlError::new(SqlClass::Interface, message));
    };
    Ok(RowCounts {
        files: count(&files)?,
        tags: count(&tags)?,
        imports: count(&imports)?,
        refs: count(&refs)?,
    })
}

fn count(cell: &Cell) -> Result<u64, SqlError> {
    let value = cell.int()?;
    u64::try_from(value)
        .map_err(|_| SqlError::new(SqlClass::Value, format!("a count of {value} rows")))
}
