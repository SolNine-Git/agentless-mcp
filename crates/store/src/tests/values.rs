use serde_json::Value;

use super::cases::cases;
use crate::value::{Cell, check_sloc};

#[test]
fn each_value_converts_as_the_reference_reader_converts_it() {
    for case in cases().values {
        let cell = decoded(&case.cell);
        let found_int = match cell.int() {
            Ok(value) => ("ok".to_owned(), Value::from(value)),
            Err(error) => ("error".to_owned(), Value::from(error.repr())),
        };
        assert_eq!(found_int, case.int, "{cell:?}");
        assert_eq!(
            (cell.text(), cell.truthy(), cell.repr()),
            (case.text, case.truthy, case.repr),
            "{cell:?}"
        );
    }
}

#[test]
fn each_sloc_value_passes_or_fails_with_the_reference_error() {
    for (cell, expected) in cases().sloc {
        let cell = decoded(&cell);
        let found = check_sloc(&cell).err().map(|error| error.repr());
        assert_eq!(found, expected, "{cell:?}");
    }
}

// A cell in the fixtures: its type, then its value. A real is "0x" and the 16 hex digits of its
// bits.
fn decoded(cell: &[Value]) -> Cell {
    let value = cell.get(1);
    match cell[0].as_str().expect("a cell type") {
        "null" => Cell::Null,
        "integer" => Cell::Integer(value.and_then(Value::as_i64).expect("an integer")),
        "real" => Cell::Real(real(value.and_then(Value::as_str).expect("real bits"))),
        "text" => Cell::Text(value.and_then(Value::as_str).expect("a text").to_owned()),
        "blob" => Cell::Blob(blob(value.and_then(Value::as_array).expect("blob bytes"))),
        other => panic!("unknown cell type {other}"),
    }
}

fn real(hex: &str) -> f64 {
    let digits = hex.strip_prefix("0x").expect("a 0x prefix");
    f64::from_bits(u64::from_str_radix(digits, 16).expect("hex digits"))
}

fn blob(bytes: &[Value]) -> Vec<u8> {
    let byte = |value: &Value| {
        let number = value.as_u64().expect("a byte");
        u8::try_from(number).expect("a byte value")
    };
    bytes.iter().map(byte).collect()
}
