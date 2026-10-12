//! The fixture section reader and the fixture decoders for the tests of every crate. They live in
//! text because it is the lowest crate with fixtures, so each crate above can depend on it.

use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;

/// Returns the section `section` of the fixture file `<manifest_dir>/fixtures/<file>.json`.
///
/// # Panics
///
/// Panics when the file cannot be read or parsed, has no such section, or the section is no `T`.
#[must_use]
pub fn cases<T: DeserializeOwned>(manifest_dir: &str, file: &str, section: &str) -> T {
    let path = Path::new(manifest_dir)
        .join("fixtures")
        .join(format!("{file}.json"));
    let shown = path.display();
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let mut sections: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{shown}: {error}"));
    let value = sections
        .remove(section)
        .unwrap_or_else(|| panic!("{shown}: no section {section}"));
    serde_json::from_value(value).unwrap_or_else(|error| panic!("{shown}: {section}: {error}"))
}

/// Returns the bits of a float that a fixture stores as "0x" and 16 hex digits.
///
/// # Panics
///
/// Panics when `hex` has no 0x prefix or its digits are not hex.
#[must_use]
pub fn bits(hex: &str) -> u64 {
    let digits = hex
        .strip_prefix("0x")
        .unwrap_or_else(|| panic!("{hex}: no 0x prefix"));
    u64::from_str_radix(digits, 16).unwrap_or_else(|error| panic!("{hex}: {error}"))
}

/// Returns the float whose bits `hex` stores, as [`bits`] reads them.
///
/// # Panics
///
/// Panics as [`bits`] does.
#[must_use]
pub fn float(hex: &str) -> f64 {
    f64::from_bits(bits(hex))
}

/// Returns the float of each item of `hexes`.
///
/// # Panics
///
/// Panics as [`bits`] does.
#[must_use]
pub fn floats(hexes: &[String]) -> Vec<f64> {
    hexes.iter().map(|hex| float(hex)).collect()
}

/// Checks `function` on each (input, output) case of the section `section`.
///
/// # Panics
///
/// Panics as [`cases`] does, and at the first input whose output differs.
pub fn check_pairs(
    manifest_dir: &str,
    file: &str,
    section: &str,
    function: impl Fn(&str) -> String,
) {
    let all: Vec<(String, String)> = cases(manifest_dir, file, section);
    for (input, expected) in all {
        assert_eq!(function(&input), expected, "{section}: input {input:?}");
    }
}

/// Checks `inside` on every code point against the runs of code points in the section `section`.
///
/// # Panics
///
/// Panics as [`cases`] does, and at the first code point where `inside` differs from the runs.
pub fn check_runs(manifest_dir: &str, file: &str, section: &str, inside: impl Fn(u32) -> bool) {
    let runs: Vec<(u32, u32)> = cases(manifest_dir, file, section);
    let mut runs = runs.iter().peekable();
    for code in 0..=0x10_ffff {
        while runs.next_if(|&&(_, last)| last < code).is_some() {}
        let expected = runs.peek().is_some_and(|&&(first, _)| first <= code);
        assert_eq!(
            inside(code),
            expected,
            "{section}: mismatch for U+{code:04X}"
        );
    }
}
