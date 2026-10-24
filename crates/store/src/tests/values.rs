use serde_json::Value;

use super::cases::cases;
use crate::SqlClass;
use crate::value::Cell;

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
fn int_of_a_text_padded_with_an_information_separator_is_refused() {
    let error = Cell::Text("\u{1c}5".to_owned())
        .int()
        .map_err(|error| error.repr());
    let expected = "ValueError(\"invalid literal for int() with base 10: '\\\\x1c5'\")";
    assert_eq!(error, Err(expected.to_owned()));
}

#[test]
fn int_of_a_utf8_blob_padded_with_a_no_break_space_is_refused() {
    let error = Cell::Blob(b"\xc2\xa05".to_vec())
        .int()
        .map_err(|error| error.class);
    assert_eq!(error, Err(SqlClass::Value));
}

#[test]
fn int_keeps_the_64_bit_range() {
    let read = |text: &str| Cell::Text(text.to_owned()).int().ok();
    assert_eq!(read("-9_223_372_036_854_775_808"), Some(i64::MIN));
    assert_eq!(read(" 9223372036854775807\n"), Some(i64::MAX));
    assert_eq!(read("9223372036854775808"), None);
    assert_eq!(read(&"9".repeat(5000)), None);
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
