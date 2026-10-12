use serde_json::Value;

use super::cases::cases;
use crate::value::{Cell, check_sloc, parsed_sloc, plain_int_list};

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

// Lists with short integers, most of them changed in one or two places, so that about half fail.
#[test]
fn the_sloc_scanner_accepts_exactly_what_the_full_parse_accepts() {
    let mut random = Random(0x9e37_79b9_7f4a_7c15);
    let mut accepted = 0;
    for _ in 0..20_000 {
        let text = random.sloc_text();
        let parsed = parsed_sloc(&Cell::Text(text.clone()));
        assert_eq!(plain_int_list(text.as_bytes()), parsed.is_ok(), "{text:?}");
        let found = check_sloc(&Cell::Text(text.clone())).map_err(|error| error.repr());
        assert_eq!(found, parsed.map_err(|error| error.repr()), "{text:?}");
        accepted += usize::from(found.is_ok());
    }
    assert!(
        (5_000..15_000).contains(&accepted),
        "{accepted} of 20000 accepted"
    );
}

#[test]
fn an_integer_past_the_scanner_digit_limit_goes_to_the_full_parse() {
    for (digits, plain) in [(4_300, true), (4_301, false)] {
        let text = format!("[-{}]", "9".repeat(digits));
        assert_eq!(plain_int_list(text.as_bytes()), plain, "{digits} digits");
        let cell = Cell::Text(text);
        let found = check_sloc(&cell).map_err(|error| error.repr());
        assert_eq!(found, parsed_sloc(&cell).map_err(|error| error.repr()));
    }
}

// A xorshift generator: the same texts on every run, with no dependency.
struct Random(u64);

const PIECES: [&str; 31] = [
    "[",
    "]",
    ",",
    " ",
    "\t",
    "\n",
    "\r",
    "-",
    "0",
    "1",
    "9",
    "00",
    "-0",
    ".",
    "e",
    "E",
    "+",
    "\"",
    "x",
    "{",
    "}",
    ":",
    "\u{feff}",
    "\u{a0}",
    "\u{b}",
    "\u{c}",
    "null",
    "true",
    "NaN",
    "Infinity",
    "-Infinity",
];

const BLANKS: [&str; 6] = ["", "", " ", "\t", "\n", "\r\n"];

impl Random {
    fn next(&mut self, below: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % u64::try_from(below).expect("a small bound"))
            .expect("a value below a usize bound")
    }

    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[self.next(from.len())]
    }

    fn sloc_text(&mut self) -> String {
        let mut text = String::from(self.pick(&BLANKS));
        text.push('[');
        for index in 0..self.next(6) {
            text.push_str(self.pick(&BLANKS));
            if index > 0 {
                text.push(',');
                text.push_str(self.pick(&BLANKS));
            }
            text.push_str(&self.integer());
        }
        text.push_str(self.pick(&BLANKS));
        text.push(']');
        text.push_str(self.pick(&BLANKS));
        for _ in 0..self.next(3) {
            self.change(&mut text);
        }
        text
    }

    fn integer(&mut self) -> String {
        let sign = if self.next(4) == 0 { "-" } else { "" };
        let digits: String = (0..=self.next(4))
            .map(|_| char::from(b'0' + u8::try_from(self.next(10)).expect("a digit")))
            .collect();
        format!("{sign}{digits}")
    }

    // Inserts a piece, removes a character, or replaces one with a piece.
    fn change(&mut self, text: &mut String) {
        let bounds: Vec<usize> = (0..=text.len())
            .filter(|&at| text.is_char_boundary(at))
            .collect();
        let at = bounds[self.next(bounds.len())];
        let piece = self.pick(&PIECES);
        let end = text[at..]
            .chars()
            .next()
            .map_or(at, |next| at + next.len_utf8());
        match self.next(3) {
            0 => text.insert_str(at, piece),
            1 => text.replace_range(at..end, ""),
            _ => text.replace_range(at..end, piece),
        }
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
