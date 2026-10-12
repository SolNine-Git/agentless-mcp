//! A fetched column value, and the conversions that the cache reader applies to it.

use pycompat::{JsonValue, PyStr, float_repr, loads, strip};
use rusqlite::Row;
use rusqlite::types::ValueRef;

use crate::{SqlClass, SqlError};

// The longest repr that the message of a refused integer text keeps.
const INT_REPR_CHARS: usize = 200;
// The longest repr that the message of a refused sloc_lines value keeps.
const SLOC_REPR_CHARS: usize = 80;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Cell {
    // A text that is not UTF-8 fails the fetch.
    pub(crate) fn read(row: &Row<'_>, index: usize) -> Result<Self, SqlError> {
        let value = row.get_ref(index).map_err(SqlError::from_sqlite)?;
        Ok(match value {
            ValueRef::Null => Self::Null,
            ValueRef::Integer(value) => Self::Integer(value),
            ValueRef::Real(value) => Self::Real(value),
            ValueRef::Text(bytes) => Self::Text(decoded(row, index, bytes)?),
            ValueRef::Blob(bytes) => Self::Blob(bytes.to_vec()),
        })
    }

    // The integer conversion: an integer, a truncated float, or a decimal text.
    pub(crate) fn int(&self) -> Result<i64, SqlError> {
        let text = match self {
            Self::Integer(value) => return Ok(*value),
            Self::Real(value) => return real_int(*value),
            Self::Null => return Err(SqlError::new(SqlClass::Type, NOT_A_NUMBER)),
            Self::Text(text) => Some(text.as_str()),
            Self::Blob(bytes) => std::str::from_utf8(bytes).ok(),
        };
        text.and_then(parse_int).ok_or_else(|| {
            let shown: String = self.repr().chars().take(INT_REPR_CHARS).collect();
            let message = format!("invalid literal for int() with base 10: {shown}");
            SqlError::new(SqlClass::Value, message)
        })
    }

    // The text conversion: a text as it is, any other value as its repr.
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            other => other.repr(),
        }
    }

    pub(crate) fn optional_text(&self) -> Option<String> {
        (*self != Self::Null).then(|| self.text())
    }

    // The truth value: NULL, zero and an empty text or blob are false.
    pub(crate) fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Integer(value) => *value != 0,
            Self::Real(value) => value.is_nan() || value.abs() > 0.0,
            Self::Text(text) => !text.is_empty(),
            Self::Blob(bytes) => !bytes.is_empty(),
        }
    }

    pub(crate) fn repr(&self) -> String {
        match self {
            Self::Null => "None".to_owned(),
            Self::Integer(value) => value.to_string(),
            Self::Real(value) => float_repr(*value),
            Self::Text(text) => PyStr::from(text.as_str()).repr(),
            Self::Blob(bytes) => bytes_repr(bytes),
        }
    }

    // Equality with a text, where `None` equals NULL alone.
    pub(crate) fn equals(&self, expected: Option<&str>) -> bool {
        match (self, expected) {
            (Self::Null, None) => true,
            (Self::Text(text), Some(expected)) => text == expected,
            _ => false,
        }
    }
}

const NOT_A_NUMBER: &str =
    "int() argument must be a string, a bytes-like object or a real number, not 'NoneType'";

// The stored value must be the JSON text of a list of integers. The values are not used.
pub(crate) fn check_sloc(raw: &Cell) -> Result<(), SqlError> {
    let parsed = match raw {
        Cell::Text(text) => Some(
            loads(text).map_err(|error| SqlError::new(SqlClass::JsonDecode, error.to_string()))?,
        ),
        _ => None,
    };
    if let Some(JsonValue::Array(items)) = &parsed
        && items.iter().all(|item| matches!(item, JsonValue::Int(_)))
    {
        return Ok(());
    }
    let shown: String = raw.repr().chars().take(SLOC_REPR_CHARS).collect();
    let message = format!("sloc_lines is not a JSON list of integers: {shown}");
    Err(SqlError::new(SqlClass::Type, message))
}

fn decoded(row: &Row<'_>, index: usize, bytes: &[u8]) -> Result<String, SqlError> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.to_owned());
    }
    let statement = row.as_ref();
    let column = statement
        .column_name(index)
        .map_err(SqlError::from_sqlite)?;
    Err(undecodable(column, bytes))
}

// The message names the column and shows the text up to its first NUL, with each byte sequence
// that is not UTF-8 replaced.
fn undecodable(column: &str, bytes: &[u8]) -> SqlError {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    let text = String::from_utf8_lossy(&bytes[..end]);
    let message = format!("Could not decode to UTF-8 column '{column}' with text '{text}'");
    SqlError::new(SqlClass::Operational, message)
}

fn real_int(value: f64) -> Result<i64, SqlError> {
    if value.is_nan() {
        let message = "cannot convert float NaN to integer";
        return Err(SqlError::new(SqlClass::Value, message));
    }
    if value.is_infinite() {
        let message = "cannot convert float infinity to integer";
        return Err(SqlError::new(SqlClass::Value, message));
    }
    // The decimal text of the truncated value avoids a lossy cast.
    let truncated = format!("{:.0}", value.trunc());
    truncated.parse().map_err(|_| {
        let message = format!("{truncated} does not fit in a 64-bit integer");
        SqlError::new(SqlClass::Value, message)
    })
}

// A decimal integer with an optional sign and surrounding whitespace. One underscore may separate
// two digits.
fn parse_int(text: &str) -> Option<i64> {
    let number = strip(text);
    let digits = number.strip_prefix(['+', '-']).unwrap_or(number);
    let valid = digits
        .split('_')
        .all(|group| !group.is_empty() && group.bytes().all(|byte| byte.is_ascii_digit()));
    valid
        .then(|| number.replace('_', "").parse().ok())
        .flatten()
}

fn bytes_repr(bytes: &[u8]) -> String {
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        b'"'
    } else {
        b'\''
    };
    let mut out = String::from("b");
    out.push(char::from(quote));
    for &byte in bytes {
        push_byte(&mut out, byte, quote);
    }
    out.push(char::from(quote));
    out
}

fn push_byte(out: &mut String, byte: u8, quote: u8) {
    match byte {
        b'\\' => out.push_str("\\\\"),
        b'\t' => out.push_str("\\t"),
        b'\n' => out.push_str("\\n"),
        b'\r' => out.push_str("\\r"),
        _ if byte == quote => {
            out.push('\\');
            out.push(char::from(byte));
        }
        b' '..=b'~' => out.push(char::from(byte)),
        _ => {
            out.push_str("\\x");
            out.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
            out.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
        }
    }
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
