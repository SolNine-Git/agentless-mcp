use std::ffi::OsString;
use std::path::PathBuf;

use app::EnvConfig;

use super::columns;

fn width_for(value: &str) -> isize {
    let vars = [(OsString::from("COLUMNS"), OsString::from(value))];
    columns(&EnvConfig::new(vars, PathBuf::from("/")))
}

// The integer rule strips no U+001C to U+001F, though they are whitespace, and it strips U+0085.
#[test]
fn columns_padded_with_an_information_separator_falls_back_to_80() {
    assert_eq!(width_for("\x1c40"), 80);
    assert_eq!(width_for("40\x1f"), 80);
    assert_eq!(width_for("\u{85}40"), 40);
}
