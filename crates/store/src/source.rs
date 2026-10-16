//! The cache-or-parse source of one file's facts and metrics. A row that describes the file now
//! is served; anything else parses the text in memory. A read never writes.

use std::sync::Arc;

use facts::{Facts, Symbol};
use syntax::{Parsed, Unparsable};

use crate::analysis::{Parse, measured};
use crate::blob::{decode_facts, decode_metrics, decode_symbols};
use crate::value::Cell;
use crate::{FactsDb, FileText, Measured, RowFailure, SqlClass, SqlError, Unmeasured, schema};

/// A value, and the warning of a stored row that could not be read and that a parse replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served<T> {
    /// The value.
    pub value: T,
    /// The warning, which the caller prints.
    pub warning: Option<RowFailure>,
}

/// The facts and the metrics of each file: from agentless.db when a row describes the file's
/// text, else from one parse of it.
pub struct FactSource {
    db: Option<FactsDb>,
    parse: Arc<Parse>,
}

impl std::fmt::Debug for FactSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FactSource")
            .field("db", &self.db)
            .finish_non_exhaustive()
    }
}

impl FactSource {
    /// Returns a source with no database: it parses every file.
    #[must_use]
    pub fn parsing() -> FactSource {
        FactSource::with(None, Arc::new(syntax::parse))
    }

    /// Returns a source that serves the rows of `db`.
    #[must_use]
    pub fn reading(db: FactsDb) -> FactSource {
        FactSource::with(Some(db), Arc::new(syntax::parse))
    }

    pub(crate) fn with(db: Option<FactsDb>, parse: Arc<Parse>) -> FactSource {
        FactSource { db, parse }
    }

    /// Returns the database that the source reads, if any.
    #[must_use]
    pub fn database(&self) -> Option<&FactsDb> {
        self.db.as_ref()
    }

    /// Returns the symbols of `file`, or the refusal of a text that does not parse.
    #[must_use]
    pub fn symbols(&self, file: &FileText) -> Served<Result<Vec<Symbol>, Unparsable>> {
        let read = |db: &FactsDb, id| {
            let [symbols] = db.cells::<1>(schema::SYMBOLS, id)?;
            decode_symbols(blob(&symbols)?).map(Some)
        };
        self.served(file, "symbol", read, |parsed| {
            facts::extract(parsed).symbols
        })
    }

    /// Returns the facts of `file`, or the refusal of a text that does not parse.
    #[must_use]
    pub fn facts(&self, file: &FileText) -> Served<Result<Facts, Unparsable>> {
        let read = |db: &FactsDb, id| {
            let [symbols, rest] = db.cells::<2>(schema::FACTS, id)?;
            decode_facts(blob(&symbols)?, blob(&rest)?).map(Some)
        };
        self.served(file, "fact", read, facts::extract)
    }

    /// Returns the metrics of `file`. A text that does not parse has the refusal as its reason.
    #[must_use]
    pub fn metrics(&self, file: &FileText) -> Served<Measured> {
        let read = |db: &FactsDb, id| {
            let [metrics] = db.cells::<1>(schema::METRICS, id)?;
            stored_metrics(&metrics)
        };
        let measure = |parsed: &Parsed| measured(parsed, &file.path);
        let Served { value, warning } = self.served(file, "metric", read, measure);
        let value = value.unwrap_or_else(|unparsable| Err(Unmeasured::of(&unparsable)));
        Served { value, warning }
    }

    /// Returns the facts and the metrics of `file` from one row or from one parse.
    #[must_use]
    pub fn facts_and_metrics(
        &self,
        file: &FileText,
    ) -> Served<Result<(Facts, Measured), Unparsable>> {
        let read = |db: &FactsDb, id| {
            let [symbols, rest, metrics] = db.cells::<3>(schema::ALL, id)?;
            let Some(measured) = stored_metrics(&metrics)? else {
                return Ok(None);
            };
            Ok(Some((
                decode_facts(blob(&symbols)?, blob(&rest)?)?,
                measured,
            )))
        };
        let derive = |parsed: &Parsed| (facts::extract(parsed), measured(parsed, &file.path));
        self.served(file, "fact", read, derive)
    }

    // The one cache-or-parse path: a stored value when the row reads, else one parse.
    fn served<T>(
        &self,
        file: &FileText,
        kind: &'static str,
        read: impl FnOnce(&FactsDb, i64) -> Result<Option<T>, SqlError>,
        derive: impl FnOnce(&Parsed) -> T,
    ) -> Served<Result<T, Unparsable>> {
        let (stored, warning) = self.stored(file, kind, read);
        let value = match stored {
            Some(value) => Ok(value),
            None => {
                (self.parse)(file.language, Arc::clone(&file.text)).map(|parsed| derive(&parsed))
            }
        };
        Served { value, warning }
    }

    fn stored<T>(
        &self,
        file: &FileText,
        kind: &'static str,
        read: impl FnOnce(&FactsDb, i64) -> Result<Option<T>, SqlError>,
    ) -> (Option<T>, Option<RowFailure>) {
        let Some(db) = &self.db else {
            return (None, None);
        };
        let found = db
            .fresh_id(file)
            .and_then(|id| id.map_or(Ok(None), |id| read(db, id)));
        match found {
            Ok(value) => (value, None),
            Err(cause) => {
                let failure = RowFailure {
                    database: db.path().to_path_buf(),
                    kind,
                    path: file.path.clone(),
                    cause: cause.repr(),
                };
                (None, Some(failure))
            }
        }
    }
}

// A NULL metrics column is a file that was not measured: the caller parses it.
fn stored_metrics(cell: &Cell) -> Result<Option<Measured>, SqlError> {
    match cell {
        Cell::Null => Ok(None),
        other => decode_metrics(blob(other)?).map(Some),
    }
}

fn blob(cell: &Cell) -> Result<&[u8], SqlError> {
    match cell {
        Cell::Blob(bytes) => Ok(bytes),
        other => {
            let message = format!("a stored blob holds {}", other.repr());
            Err(SqlError::new(SqlClass::Type, message))
        }
    }
}
