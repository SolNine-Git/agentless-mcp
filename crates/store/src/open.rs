//! The open of a tag cache database: the presence check, the read-only connection, and the index
//! checks that decide whether the database is used.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Params, Row};

use crate::freshness::rejection;
use crate::value::Cell;
use crate::{IndexMeta, Refusal, SCHEMA_VERSION, SqlClass, SqlError};

// A writer that holds an exclusive lock delays a reader by at most this long.
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const HAS_TABLE: &str = "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?";
const META: &str =
    "SELECT schema_version, repo_root, generation_tree_oid, head_sha FROM meta WHERE id = 1";
const ENTRIES: &str = "SELECT files.path, files.sha256, files.grammar_version, files.id, \
    file_metrics.stamp FROM files LEFT JOIN file_metrics ON file_metrics.file_id = files.id";
const COUNT_FILES: &str = "SELECT COUNT(*) FROM files";
const COUNT_TAGS: &str = "SELECT COUNT(*) FROM tags";
const COUNT_IMPORTS: &str = "SELECT COUNT(*) FROM imports";
const COUNT_REFS: &str = "SELECT COUNT(*) FROM refs";

/// What the open of a repository's database found: the tag cache by default, or agentless.db.
#[derive(Debug)]
pub enum Source<D = TagsDb> {
    /// The caller asked to parse every file itself.
    Bypassed,
    /// No database exists at the path.
    Absent,
    /// A database exists and is not used.
    Refused(Refusal),
    /// A database that this build reads, with one read snapshot held open.
    Usable(D),
}

/// An open database that passed the index checks. The drop ends its read snapshot.
#[derive(Debug)]
pub struct TagsDb {
    pub(crate) connection: Connection,
    pub(crate) database: PathBuf,
    pub(crate) entries: HashMap<String, Entry>,
    generation: String,
}

/// The row count of each table that the capabilities report reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowCounts {
    /// The indexed files.
    pub files: u64,
    /// The symbol rows.
    pub tags: u64,
    /// The import rows.
    pub imports: u64,
    /// The reference rows.
    pub refs: u64,
}

#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) digest: String,
    pub(crate) grammar_version: String,
    pub(crate) file_id: i64,
}

/// Opens the database at `database` for the repository `repo_root`, unless `no_cache` asks to
/// parse every file instead. The database is opened read-only, and a refused one is not changed.
#[must_use]
pub fn open_source(database: &Path, repo_root: &str, no_cache: bool) -> Source {
    if no_cache {
        return Source::Bypassed;
    }
    open(database, repo_root, BUSY_TIMEOUT)
}

pub(crate) fn open(database: &Path, repo_root: &str, busy: Duration) -> Source {
    found(database, || TagsDb::open(database, repo_root, busy))
}

// The checks before either database opens: a missing file is absent, and a path that cannot be
// read or is a directory is refused and left in place.
pub(crate) fn found<D>(database: &Path, open: impl FnOnce() -> Result<D, Refusal>) -> Source<D> {
    match std::fs::metadata(database) {
        Err(error) if absent(&error) => Source::Absent,
        Err(error) => Source::Refused(Refusal::Unreadable(SqlError::new(
            SqlClass::Operational,
            ::io::strerror(&error),
        ))),
        // SQLite reports a directory as an I/O error, and the note names the failed open instead.
        Ok(found) if found.is_dir() => Source::Refused(Refusal::Unreadable(SqlError::new(
            SqlClass::Operational,
            "unable to open database file",
        ))),
        Ok(_) => match open() {
            Ok(opened) => Source::Usable(opened),
            Err(refusal) => Source::Refused(refusal),
        },
    }
}

// A missing file, a missing directory, a symlink loop or a bad descriptor means no database.
fn absent(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENOENT | libc::ENOTDIR | libc::EBADF | libc::ELOOP)
    )
}

impl TagsDb {
    fn open(database: &Path, repo_root: &str, busy: Duration) -> Result<Self, Refusal> {
        let connection = connect(database, busy)?;
        let (meta, entries) = read_index(&connection, repo_root)?;
        Ok(Self {
            connection,
            database: database.to_path_buf(),
            entries,
            generation: meta.generation,
        })
    }

    /// Returns the generation that the database was built at: the git tree id, or a `nogit:`
    /// manifest digest.
    #[must_use]
    pub fn generation(&self) -> &str {
        &self.generation
    }

    /// Counts the rows of the files, tags, imports and refs tables.
    ///
    /// # Errors
    ///
    /// Returns the [`SqlError`] of a count that fails.
    pub fn row_counts(&self) -> Result<RowCounts, SqlError> {
        Ok(RowCounts {
            files: self.count(COUNT_FILES)?,
            tags: self.count(COUNT_TAGS)?,
            imports: self.count(COUNT_IMPORTS)?,
            refs: self.count(COUNT_REFS)?,
        })
    }

    fn count(&self, sql: &str) -> Result<u64, SqlError> {
        let rows = fetch::<1>(&self.connection, sql, (), 1)?;
        let [cell] = rows
            .into_iter()
            .next()
            .ok_or_else(|| SqlError::new(SqlClass::Interface, format!("{sql} gave no row")))?;
        let value = cell.int()?;
        u64::try_from(value)
            .map_err(|_| SqlError::new(SqlClass::Value, format!("{sql} gave {value}")))
    }
}

pub(crate) fn connect(database: &Path, busy: Duration) -> Result<Connection, SqlError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    connect_with(database, flags, busy)
}

pub(crate) fn connect_with(
    database: &Path,
    flags: OpenFlags,
    busy: Duration,
) -> Result<Connection, SqlError> {
    let connection =
        Connection::open_with_flags(database, flags).map_err(|error| opened(error, database))?;
    connection
        .busy_timeout(busy)
        .map_err(SqlError::from_sqlite)?;
    Ok(connection)
}

// The binding appends ": <path>" to the message of a failed open.
fn opened(error: rusqlite::Error, database: &Path) -> SqlError {
    let found = SqlError::from_sqlite(error);
    let suffix = format!(": {}", database.to_string_lossy());
    match found.message.strip_suffix(&suffix) {
        Some(message) => SqlError::new(found.class, message),
        None => found,
    }
}

// One deferred read transaction pins every later read to the same snapshot.
fn read_index(
    connection: &Connection,
    repo_root: &str,
) -> Result<(IndexMeta, HashMap<String, Entry>), Refusal> {
    connection
        .execute("BEGIN", ())
        .map_err(SqlError::from_sqlite)?;
    let (meta, _head_sha) = read_meta(connection, META)?.ok_or(Refusal::NoIndex)?;
    if let Some(refusal) = rejection(&meta, SCHEMA_VERSION, repo_root) {
        return Err(refusal);
    }
    let entries = load_entries(connection)?;
    Ok((meta, entries))
}

// Asked directly: an error around the SELECT would also cover a disk that fails.
pub(crate) fn has_meta(connection: &Connection) -> Result<bool, SqlError> {
    Ok(!fetch::<1>(connection, HAS_TABLE, ["meta"], 1)?.is_empty())
}

// `sql` selects the schema version, the repository root, the generation, and one more column,
// whose cell comes back as it is. None means no meta table or no meta row.
pub(crate) fn read_meta(
    connection: &Connection,
    sql: &str,
) -> Result<Option<(IndexMeta, Cell)>, SqlError> {
    if !has_meta(connection)? {
        return Ok(None);
    }
    let Some([schema, repo_root, generation, more]) =
        fetch::<4>(connection, sql, (), 1)?.into_iter().next()
    else {
        return Ok(None);
    };
    let meta = IndexMeta {
        schema: schema.int()?,
        repo_root: repo_root.text(),
        generation: generation.text(),
    };
    Ok(Some((meta, more)))
}

fn load_entries(connection: &Connection) -> Result<HashMap<String, Entry>, SqlError> {
    let rows = fetch::<5>(connection, ENTRIES, (), usize::MAX)?;
    rows.into_iter()
        .map(|[path, digest, grammar_version, file_id, _stamp]| {
            let entry = Entry {
                digest: digest.text(),
                grammar_version: grammar_version.text(),
                file_id: file_id.int()?,
            };
            Ok((path.text(), entry))
        })
        .collect()
}

// Runs `sql` and returns the cells of at most `limit` rows. Each fetched row is decoded whole.
pub(crate) fn fetch<const N: usize>(
    connection: &Connection,
    sql: &str,
    params: impl Params,
    limit: usize,
) -> Result<Vec<[Cell; N]>, SqlError> {
    let mut statement = connection
        .prepare_cached(sql)
        .map_err(SqlError::from_sqlite)?;
    let mut rows = statement.query(params).map_err(SqlError::from_sqlite)?;
    let mut found = Vec::new();
    while found.len() < limit {
        let Some(row) = rows.next().map_err(SqlError::from_sqlite)? else {
            break;
        };
        found.push(cells(row)?);
    }
    Ok(found)
}

fn cells<const N: usize>(row: &Row<'_>) -> Result<[Cell; N], SqlError> {
    let cells = (0..N)
        .map(|index| Cell::read(row, index))
        .collect::<Result<Vec<_>, _>>()?;
    cells.try_into().map_err(|cells: Vec<Cell>| {
        let message = format!("{} columns where the query reads {N}", cells.len());
        SqlError::new(SqlClass::Interface, message)
    })
}
