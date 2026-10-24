//! The three blobs of one agentless.db row: the symbols, the other facts, and the metrics.

use facts::{Facts, Import, Ref, Symbol};
use kernel::Span;
use serde::{Deserialize, Serialize};

use crate::value::Cell;
use crate::{Analysis, Measured, SqlClass, SqlError};

/// One analysis as its row stores it: the counts, and the blobs or the first encode error.
#[derive(Debug)]
pub struct Encoded {
    pub(crate) symbols: usize,
    pub(crate) imports: usize,
    pub(crate) refs: usize,
    pub(crate) blobs: Result<Blobs, SqlError>,
}

#[derive(Debug)]
pub(crate) struct Blobs {
    pub(crate) metrics: Option<Vec<u8>>,
    pub(crate) symbols: Vec<u8>,
    pub(crate) rest: Vec<u8>,
}

// Every Facts field but the symbols, which have a blob of their own so that a read of the
// symbols never decodes the references.
#[derive(Serialize)]
struct Rest<'a> {
    imports: &'a [Import],
    refs: &'a [Ref],
    errors: &'a [Span],
}

#[derive(Deserialize)]
struct OwnedRest {
    imports: Vec<Import>,
    refs: Vec<Ref>,
    errors: Vec<Span>,
}

/// Encodes `analysis` for its row. A blob that does not encode fails the write of the row.
#[must_use]
pub fn encode(analysis: &Analysis) -> Encoded {
    let facts = &analysis.facts;
    Encoded {
        symbols: facts.symbols.len(),
        imports: facts.imports.len(),
        refs: facts.refs.len(),
        blobs: blobs(analysis),
    }
}

// The metrics, then the symbols, then the rest: the first that fails names the error.
fn blobs(analysis: &Analysis) -> Result<Blobs, SqlError> {
    Ok(Blobs {
        metrics: analysis.metrics.as_ref().map(encode_metrics).transpose()?,
        symbols: encode_symbols(&analysis.facts.symbols)?,
        rest: encode_rest(&analysis.facts)?,
    })
}

fn encode_symbols(symbols: &[Symbol]) -> Result<Vec<u8>, SqlError> {
    encoded(&symbols)
}

fn encode_rest(facts: &Facts) -> Result<Vec<u8>, SqlError> {
    encoded(&Rest {
        imports: &facts.imports,
        refs: &facts.refs,
        errors: &facts.errors,
    })
}

fn encode_metrics(measured: &Measured) -> Result<Vec<u8>, SqlError> {
    encoded(measured)
}

pub(crate) fn decode_symbols(bytes: &[u8]) -> Result<Vec<Symbol>, SqlError> {
    decoded(bytes)
}

pub(crate) fn decode_facts(symbols: &[u8], rest: &[u8]) -> Result<Facts, SqlError> {
    let OwnedRest {
        imports,
        refs,
        errors,
    } = decoded(rest)?;
    Ok(Facts {
        symbols: decode_symbols(symbols)?,
        imports,
        refs,
        errors,
    })
}

pub(crate) fn decode_metrics(bytes: &[u8]) -> Result<Measured, SqlError> {
    decoded(bytes)
}

// A NULL metrics column is a file that was not measured: the caller parses it.
pub(crate) fn stored_metrics(cell: &Cell) -> Result<Option<Measured>, SqlError> {
    match cell {
        Cell::Null => Ok(None),
        other => decode_metrics(blob(other)?).map(Some),
    }
}

pub(crate) fn blob(cell: &Cell) -> Result<&[u8], SqlError> {
    match cell {
        Cell::Blob(bytes) => Ok(bytes),
        other => {
            let message = format!("a stored blob holds {}", other.repr());
            Err(SqlError::new(SqlClass::Type, message))
        }
    }
}

// The values hold no map with a key that is not text, so the encoder has no failure to give; a
// failure is still a refused write, never a panic.
fn encoded(value: &impl Serialize) -> Result<Vec<u8>, SqlError> {
    serde_json::to_vec(value).map_err(|error| SqlError::new(SqlClass::Interface, error.to_string()))
}

fn decoded<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, SqlError> {
    serde_json::from_slice(bytes)
        .map_err(|error| SqlError::new(SqlClass::JsonDecode, error.to_string()))
}
