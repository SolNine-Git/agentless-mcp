//! The stored clone blocks of one file, served only while the row describes the caller's text.

use std::fmt;
use std::path::PathBuf;

use kernel::Key;
use metrics::Block;
use pycompat::PyStr;
use syntax::{Language, PACK_VERSION};

use crate::open::fetch;
use crate::value::{Cell, check_sloc};
use crate::{SqlClass, SqlError, TagsDb, sha256_hex};

const METRICS: &str = "SELECT stamp, language, units_reason, unit_count, blocks_reason, \
    block_count, sloc_lines FROM file_metrics WHERE file_id = ?";
const BLOCKS: &str = "SELECT start_line, end_line, type2_key, exact_key, is_function, tokens \
    FROM blocks WHERE file_id = ? ORDER BY ordinal";

/// What the database holds for one file's clone blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The stored blocks, in stored order.
    Hit(Vec<Block>),
    /// The file could not be measured when it was indexed, for the stored reason.
    Unmeasured(String),
    /// The database holds no row for this text. The caller measures the file.
    Miss,
    /// The rows could not be read. The caller measures the file and prints the warning.
    Unreadable(RowFailure),
}

/// Block rows that could not be read. It displays as the warning line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowFailure {
    database: PathBuf,
    path: String,
    cause: String,
}

impl fmt::Display for RowFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "tag cache {}: block rows for {} are unreadable ({}); parsing the file instead",
            self.database.display(),
            self.path,
            self.cause
        )
    }
}

struct MetricRow {
    blocks_reason: Option<String>,
    block_count: i64,
    sloc_lines: Cell,
}

struct StoredBlock {
    start_line: i64,
    end_line: i64,
    key: String,
    exact: String,
    function: bool,
    tokens: i64,
}

impl TagsDb {
    /// Returns the stored clone blocks of `path` when the database indexed exactly `text`, with
    /// the installed grammars and the current metrics stamp of `language`.
    #[must_use]
    pub fn blocks_for(&self, path: &str, text: &str, language: Language) -> Lookup {
        let Some(file_id) = self.fresh_file_id(path, text) else {
            return Lookup::Miss;
        };
        self.stored_blocks(file_id, language)
            .unwrap_or_else(|cause| {
                Lookup::Unreadable(RowFailure {
                    database: self.database.clone(),
                    path: path.to_owned(),
                    cause: cause.repr(),
                })
            })
    }

    fn fresh_file_id(&self, path: &str, text: &str) -> Option<i64> {
        let entry = self.entries.get(path)?;
        let fresh =
            entry.grammar_version == PACK_VERSION && entry.digest == sha256_hex(text.as_bytes());
        fresh.then_some(entry.file_id)
    }

    fn stored_blocks(&self, file_id: i64, language: Language) -> Result<Lookup, SqlError> {
        let Some(metric) = self.metric_row(file_id, language)? else {
            return Ok(Lookup::Miss);
        };
        if let Some(reason) = metric.blocks_reason {
            return Ok(Lookup::Unmeasured(reason));
        }
        let stored = fetch::<6>(&self.connection, BLOCKS, [file_id], usize::MAX)?
            .into_iter()
            .map(stored_block)
            .collect::<Result<Vec<_>, _>>()?;
        counted(stored.len(), metric.block_count)?;
        check_sloc(&metric.sloc_lines)?;
        let blocks = stored.iter().map(block).collect::<Result<_, _>>()?;
        Ok(Lookup::Hit(blocks))
    }

    fn metric_row(&self, file_id: i64, language: Language) -> Result<Option<MetricRow>, SqlError> {
        let Some(row) = fetch::<7>(&self.connection, METRICS, [file_id], 1)?.pop() else {
            return Ok(None);
        };
        let [stamp, stored, _, unit_count, reason, count, sloc_lines] = row;
        if !stamp.equals(metrics::stamp(language)) || !stored.equals(Some(language.name())) {
            return Ok(None);
        }
        // The unit count is converted for the failure it can report. The blocks do not use it.
        unit_count.int()?;
        Ok(Some(MetricRow {
            blocks_reason: reason.optional_text(),
            block_count: count.int()?,
            sloc_lines,
        }))
    }
}

fn stored_block(row: [Cell; 6]) -> Result<StoredBlock, SqlError> {
    let [start_line, end_line, key, exact, function, tokens] = row;
    Ok(StoredBlock {
        start_line: start_line.int()?,
        end_line: end_line.int()?,
        key: key.text(),
        exact: exact.text(),
        function: function.truthy(),
        tokens: tokens.int()?,
    })
}

fn counted(found: usize, expected: i64) -> Result<(), SqlError> {
    if i64::try_from(found).is_ok_and(|found| found == expected) {
        return Ok(());
    }
    let message = format!("{found} block rows where the metrics row records {expected}");
    Err(SqlError::new(SqlClass::Value, message))
}

// A block line, a token count, or a key that the block type cannot hold refuses the rows.
fn block(stored: &StoredBlock) -> Result<Block, SqlError> {
    Ok(Block {
        start_line: number(stored.start_line, "start_line")?,
        end_line: number(stored.end_line, "end_line")?,
        key: key(&stored.key, "type2_key")?,
        exact: key(&stored.exact, "exact_key")?,
        function: stored.function,
        tokens: number(stored.tokens, "tokens")?,
    })
}

fn number(value: i64, column: &str) -> Result<u32, SqlError> {
    u32::try_from(value).map_err(|_| {
        let message = format!("{column} {value} is not between 0 and {}", u32::MAX);
        SqlError::new(SqlClass::Value, message)
    })
}

fn key(text: &str, column: &str) -> Result<Key, SqlError> {
    text.parse().map_err(|_| {
        let shown = PyStr::from(text).repr();
        let message = format!("{column} {shown} is not 64 lowercase hexadecimal characters");
        SqlError::new(SqlClass::Value, message)
    })
}
