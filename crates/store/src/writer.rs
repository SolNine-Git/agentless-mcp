//! The one writer of agentless.db: the owner-only directory, the lock, the open that replaces a
//! database whose tables do not read, and the one transaction of an index run.

use std::path::{Path, PathBuf};

use io::{LockError, LockGuard};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};

use crate::discard::{FACTS_LOCK_NAME, remove_database};
use crate::freshness::facts_schema_rejection;
use crate::open::{BUSY_TIMEOUT, connect_with, fetch};
use crate::plan::{Entry, NewRow};
use crate::reader::{row_counts, stored_schema};
use crate::value::Cell;
use crate::warnings::{Failed, unreclaimed};
use crate::{
    FACTS_SCHEMA_VERSION, Previous, Refusal, RowCounts, Settled, SqlError, analysis, schema,
};

/// The writer of one repository's agentless.db. It holds agentless.lock until it drops.
#[derive(Debug)]
pub struct Writer {
    // Declared before the lock, so the connection closes before the lock is released.
    connection: Connection,
    previous: Previous,
    _lock: LockGuard,
}

/// Why no writer opened.
#[derive(Debug)]
pub enum WriteError {
    /// Another index run holds the lock, or the file system cannot lock.
    Locked {
        /// The directory of the lock file.
        directory: PathBuf,
    },
    /// The directory or the lock file could not be created, or a database file not removed.
    Io(std::io::Error),
    /// The database could not be opened or read.
    Sql(SqlError),
    /// The database belongs to a newer build. Nothing was changed.
    Refused(Refusal),
}

impl From<SqlError> for WriteError {
    fn from(error: SqlError) -> Self {
        Self::Sql(error)
    }
}

/// The meta row that one run records.
#[derive(Clone, Copy, Debug)]
pub struct NewMeta<'a> {
    /// The resolved repository root.
    pub repo_root: &'a str,
    /// The generation of the repository now.
    pub generation: &'a str,
    /// The time of the run, in seconds since the epoch.
    pub created: i64,
}

impl Writer {
    /// Creates the database directory owner-only, takes agentless.lock without waiting, and
    /// opens `database`. A database whose tables do not read is removed and created again, one
    /// of an older layout is migrated, and one of a newer layout is refused. Returns the writer
    /// or why there is none, and the warnings that came before it.
    pub fn open(database: &Path) -> (Result<Writer, WriteError>, Vec<String>) {
        let directory = database.parent().unwrap_or(Path::new(""));
        let mut warnings: Vec<String> = match io::create_private_dir(directory) {
            Ok(failed) => failed
                .iter()
                .map(|(path, error)| Failed::Restricting(path).warning(error))
                .collect(),
            Err(error) => return (Err(WriteError::Io(error)), Vec::new()),
        };
        let opened = locked(database).and_then(|lock| opened(database, lock));
        match opened {
            Ok((writer, reclaim)) => {
                warnings.extend(reclaim);
                (Ok(writer), warnings)
            }
            Err(error) => (Err(error), warnings),
        }
    }

    /// Returns what the database held when the writer opened it.
    #[must_use]
    pub fn previous(&self) -> &Previous {
        &self.previous
    }

    /// Writes `settled` and `meta` in one transaction: the deletes, the rows in path order, then
    /// the meta row. Returns the counts after the write, and releases the lock.
    ///
    /// # Errors
    ///
    /// Returns the [`SqlError`] of a statement that fails, or of the first row in path order
    /// whose blobs did not encode; nothing is written then.
    pub fn write(mut self, settled: &Settled, meta: NewMeta<'_>) -> Result<RowCounts, SqlError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(SqlError::from_sqlite)?;
        for path in &settled.delete {
            run(&transaction, schema::DELETE, [path])?;
        }
        for row in &settled.rows {
            insert(&transaction, row)?;
        }
        write_meta(&transaction, meta)?;
        let counts = row_counts(&transaction)?;
        transaction.commit().map_err(SqlError::from_sqlite)?;
        Ok(counts)
    }
}

fn locked(database: &Path) -> Result<LockGuard, WriteError> {
    match io::try_lock(&database.with_file_name(FACTS_LOCK_NAME)) {
        Ok(lock) => Ok(lock),
        Err(LockError::Held | LockError::Refused(_)) => Err(WriteError::Locked {
            directory: database.parent().unwrap_or(Path::new("")).to_path_buf(),
        }),
        Err(LockError::Open(error)) => Err(WriteError::Io(error)),
    }
}

// A file that is not a database fails the connect, and the run stops there. A database whose
// tables do not read is removed under the lock and created again.
fn opened(database: &Path, lock: LockGuard) -> Result<(Writer, Option<String>), WriteError> {
    let connection = connect_rw(database)?;
    refuse_newer(&connection)?;
    let ready = ensure_schema(&connection);
    let (connection, reclaim) = if let Ok(reclaim) = ready {
        (connection, reclaim)
    } else {
        rebuilt(database, connection)?
    };
    let previous = load_previous(&connection)?;
    let writer = Writer {
        connection,
        previous,
        _lock: lock,
    };
    Ok((writer, reclaim))
}

// A newer layout belongs to the newer build that wrote it, so no migration goes down. A schema
// that does not read is left to ensure_schema, whose failure builds the database again.
fn refuse_newer(connection: &Connection) -> Result<(), WriteError> {
    let found = stored_schema(connection).ok().flatten();
    match found.and_then(facts_schema_rejection) {
        Some(refusal @ Refusal::NewerSchema { .. }) => Err(WriteError::Refused(refusal)),
        _ => Ok(()),
    }
}

fn rebuilt(
    database: &Path,
    connection: Connection,
) -> Result<(Connection, Option<String>), WriteError> {
    drop(connection);
    remove_database(database).map_err(WriteError::Io)?;
    let connection = connect_rw(database)?;
    let reclaim = ensure_schema(&connection)?;
    Ok((connection, reclaim))
}

fn connect_rw(database: &Path) -> Result<Connection, SqlError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = connect_with(database, flags, BUSY_TIMEOUT)?;
    connection
        .query_row("PRAGMA journal_mode=WAL", (), |_| Ok(()))
        .map_err(SqlError::from_sqlite)?;
    // In WAL mode NORMAL can lose the last run to a power loss but keeps the file whole, and the
    // next index run builds the lost rows again from the files.
    connection
        .execute_batch("PRAGMA synchronous=NORMAL")
        .map_err(SqlError::from_sqlite)?;
    Ok(connection)
}

// Creates the tables, and drops them first when the database records another layout. Returns
// the warning of a migration whose freed pages stay in the file.
fn ensure_schema(connection: &Connection) -> Result<Option<String>, SqlError> {
    let stale =
        stored_schema(connection)?.is_some_and(|found| found != i64::from(FACTS_SCHEMA_VERSION));
    let transaction = connection
        .unchecked_transaction()
        .map_err(SqlError::from_sqlite)?;
    if stale {
        batch(&transaction, schema::DROP)?;
    }
    batch(&transaction, schema::CREATE)?;
    transaction.commit().map_err(SqlError::from_sqlite)?;
    Ok(stale.then(|| reclaim(connection)).flatten())
}

// A dropped table frees pages inside the file. VACUUM returns them, and needs room for a copy.
fn reclaim(connection: &Connection) -> Option<String> {
    let error = connection.execute_batch("VACUUM").err()?;
    Some(unreclaimed(&SqlError::from_sqlite(error)))
}

fn load_previous(connection: &Connection) -> Result<Previous, SqlError> {
    let rows = fetch::<4>(connection, schema::ENTRIES, (), usize::MAX)?;
    let entries = rows
        .into_iter()
        .map(|[path, language, digest, measured]| {
            let entry = Entry {
                language: language.text(),
                digest: bytes(digest),
                measured: measured.truthy(),
            };
            (path.text(), entry)
        })
        .collect();
    let analysis = fetch::<1>(connection, schema::ANALYSIS, (), 1)?
        .into_iter()
        .next()
        .map(|[analysis]| bytes(analysis));
    Ok(Previous { entries, analysis })
}

// A value that is not a blob matches no digest.
fn bytes(cell: Cell) -> Vec<u8> {
    match cell {
        Cell::Blob(bytes) => bytes,
        _ => Vec::new(),
    }
}

fn insert(transaction: &Transaction<'_>, row: &NewRow) -> Result<(), SqlError> {
    let encoded = &row.encoded;
    let blobs = encoded.blobs.as_ref().map_err(SqlError::clone)?;
    let values = params![
        row.file.path,
        row.file.language.name(),
        row.digest.bytes().as_slice(),
        count(encoded.symbols),
        count(encoded.imports),
        count(encoded.refs),
        blobs.metrics,
        blobs.symbols,
        blobs.rest,
    ];
    run(transaction, schema::INSERT, values)
}

fn write_meta(transaction: &Transaction<'_>, meta: NewMeta<'_>) -> Result<(), SqlError> {
    run(transaction, schema::CLEAR_META, ())?;
    let analysis = analysis();
    let values = params![
        FACTS_SCHEMA_VERSION,
        meta.repo_root,
        meta.generation,
        analysis.bytes().as_slice(),
        meta.created,
    ];
    run(transaction, schema::WRITE_META, values)
}

// SQLite integers are signed 64-bit; no in-memory count reaches the limit.
fn count(length: usize) -> i64 {
    i64::try_from(length).unwrap_or(i64::MAX)
}

fn run(
    transaction: &Transaction<'_>,
    sql: &str,
    values: impl rusqlite::Params,
) -> Result<(), SqlError> {
    transaction
        .prepare_cached(sql)
        .and_then(|mut statement| statement.execute(values))
        .map(drop)
        .map_err(SqlError::from_sqlite)
}

fn batch(transaction: &Transaction<'_>, sql: &str) -> Result<(), SqlError> {
    transaction
        .execute_batch(sql)
        .map_err(SqlError::from_sqlite)
}
