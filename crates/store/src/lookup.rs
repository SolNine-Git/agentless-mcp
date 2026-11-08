//! The stored clone blocks, or the symbols and metrics, of one file, served only while the row
//! describes the caller's text.

use std::fmt;
use std::path::PathBuf;

use facts::Symbol;
use metrics::{Block, FileMetrics};
use syntax::Language;

use crate::blob::{blob, decode_symbols, stored_metrics};
use crate::{FactsDb, RowKey, SqlError, Unmeasured, schema};

/// What the database holds for one file's clone blocks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// The stored blocks, in stored order.
    Hit(Vec<Block>),
    /// The file could not be measured when it was indexed, for the stored clone index reason.
    Unmeasured(String),
    /// The database holds no row for this text, or no reason the clone index can use. The caller
    /// measures the file.
    Miss,
    /// The rows could not be read. The caller measures the file and prints the warning.
    Unreadable(RowFailure),
}

/// What the database holds for one file's functions: its symbols and its measured units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitLookup {
    /// The stored symbols and metrics.
    Hit(Vec<Symbol>, FileMetrics),
    /// The database holds no row for this text, or the file was not measured. The caller parses
    /// the file.
    Miss,
    /// The rows could not be read. The caller parses the file and prints the warning.
    Unreadable(RowFailure),
}

/// Stored rows of one file that could not be read. It displays as the warning line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowFailure {
    pub(crate) database: PathBuf,
    // The word for what the rows hold, such as "block".
    pub(crate) kind: &'static str,
    pub(crate) path: String,
    pub(crate) cause: String,
}

impl fmt::Display for RowFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "tag cache {}: {} rows for {} are unreadable ({}); parsing the file instead",
            self.database.display(),
            self.kind,
            self.path,
            self.cause
        )
    }
}

impl FactsDb {
    /// Returns the clone blocks that the row of `path` stores, when the row describes `text` in
    /// `language` now.
    #[must_use]
    pub fn blocks_for(&self, path: &str, text: &str, language: Language) -> Lookup {
        self.stored_blocks(path, text, language)
            .unwrap_or_else(|cause| {
                Lookup::Unreadable(RowFailure {
                    database: self.path().to_path_buf(),
                    kind: "block",
                    path: path.to_owned(),
                    cause: cause.repr(),
                })
            })
    }

    /// Returns the symbols and the metrics that the row of `path` stores, when the row describes
    /// `text` in `language` now and the file was measured.
    #[must_use]
    pub fn units_for(&self, path: &str, text: &str, language: Language) -> UnitLookup {
        self.stored_units(path, text, language)
            .unwrap_or_else(|cause| {
                UnitLookup::Unreadable(RowFailure {
                    database: self.path().to_path_buf(),
                    kind: "unit",
                    path: path.to_owned(),
                    cause: cause.repr(),
                })
            })
    }

    fn stored_units(
        &self,
        path: &str,
        text: &str,
        language: Language,
    ) -> Result<UnitLookup, SqlError> {
        let Some(id) = self.fresh_row(path, &RowKey::of(language, text))? else {
            return Ok(UnitLookup::Miss);
        };
        let [metrics] = self.cells::<1>(schema::METRICS, id)?;
        let Some(Ok(measured)) = stored_metrics(&metrics)? else {
            return Ok(UnitLookup::Miss);
        };
        let [symbols] = self.cells::<1>(schema::SYMBOLS, id)?;
        Ok(UnitLookup::Hit(decode_symbols(blob(&symbols)?)?, measured))
    }

    fn stored_blocks(
        &self,
        path: &str,
        text: &str,
        language: Language,
    ) -> Result<Lookup, SqlError> {
        let Some(id) = self.fresh_row(path, &RowKey::of(language, text))? else {
            return Ok(Lookup::Miss);
        };
        let [metrics] = self.cells::<1>(schema::METRICS, id)?;
        Ok(match stored_metrics(&metrics)? {
            Some(Ok(measured)) => Lookup::Hit(measured.blocks),
            Some(Err(Unmeasured {
                blocks_reason: Some(reason),
                ..
            })) => Lookup::Unmeasured(reason),
            Some(Err(_)) | None => Lookup::Miss,
        })
    }
}
