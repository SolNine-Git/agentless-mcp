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
            .unwrap_or_else(|cause| Lookup::Unreadable(self.failure("block", path, &cause)))
    }

    /// Returns the symbols and the metrics that the row of `path` stores, or `None` when no row
    /// describes `text` in `language` now or the file was not measured.
    ///
    /// # Errors
    ///
    /// Returns the [`RowFailure`] of rows that could not be read.
    pub fn units_for(
        &self,
        path: &str,
        text: &str,
        language: Language,
    ) -> Result<Option<(Vec<Symbol>, FileMetrics)>, RowFailure> {
        (self.stored_units(path, text, language))
            .map_err(|cause| self.failure("unit", path, &cause))
    }

    // The rows of `path` that hold `kind` did not read for `cause`.
    fn failure(&self, kind: &'static str, path: &str, cause: &SqlError) -> RowFailure {
        RowFailure {
            database: self.path().to_path_buf(),
            kind,
            path: path.to_owned(),
            cause: cause.repr(),
        }
    }

    fn stored_units(
        &self,
        path: &str,
        text: &str,
        language: Language,
    ) -> Result<Option<(Vec<Symbol>, FileMetrics)>, SqlError> {
        let Some(id) = self.fresh_row(path, &RowKey::of(language, text))? else {
            return Ok(None);
        };
        let [metrics] = self.cells::<1>(schema::METRICS, id)?;
        let Some(Ok(measured)) = stored_metrics(&metrics)? else {
            return Ok(None);
        };
        let [symbols] = self.cells::<1>(schema::SYMBOLS, id)?;
        Ok(Some((decode_symbols(blob(&symbols)?)?, measured)))
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
