//! The connection to a database file: the read-only open, the meta row, and the fetch of rows.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Params, Row};

use crate::value::Cell;
use crate::{IndexMeta, SqlClass, SqlError};

// A writer that holds an exclusive lock delays a reader by at most this long.
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

const HAS_TABLE: &str = "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?";

/// The file, symbol, import and reference counts of a cache: what capabilities and the index
/// report print.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowCounts {
    /// The indexed files.
    pub files: u64,
    /// The symbols.
    pub tags: u64,
    /// The imports.
    pub imports: u64,
    /// The references.
    pub refs: u64,
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
    // The caller knows whether the cell after the generation is an analysis digest.
    let meta = IndexMeta {
        schema: schema.int()?,
        repo_root: repo_root.text(),
        generation: generation.text(),
        analysis: None,
    };
    Ok(Some((meta, more)))
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
