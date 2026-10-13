//! The three blobs of one agentless.db row: the symbols, the other facts, and the metrics.

use facts::{Facts, Import, Ref, Symbol};
use kernel::Span;
use serde::{Deserialize, Serialize};

use crate::{Measured, SqlClass, SqlError};

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

pub(crate) fn encode_symbols(symbols: &[Symbol]) -> Result<Vec<u8>, SqlError> {
    encoded(&symbols)
}

pub(crate) fn encode_rest(facts: &Facts) -> Result<Vec<u8>, SqlError> {
    encoded(&Rest {
        imports: &facts.imports,
        refs: &facts.refs,
        errors: &facts.errors,
    })
}

pub(crate) fn encode_metrics(measured: &Measured) -> Result<Vec<u8>, SqlError> {
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

// The values hold no map with a key that is not text, so the encoder has no failure to give; a
// failure is still a refused write, never a panic.
fn encoded(value: &impl Serialize) -> Result<Vec<u8>, SqlError> {
    serde_json::to_vec(value).map_err(|error| SqlError::new(SqlClass::Interface, error.to_string()))
}

fn decoded<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, SqlError> {
    serde_json::from_slice(bytes)
        .map_err(|error| SqlError::new(SqlClass::JsonDecode, error.to_string()))
}
