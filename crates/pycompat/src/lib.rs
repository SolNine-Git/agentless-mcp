//! Text and number rules that the tokenizer, the checks and the renderers share: string
//! splitting, float formatting, the JSON form, and source line counts.

mod difflib;
mod float;
mod json;
mod loads;
mod order;
mod oserror;
mod printable;
mod pystr;
mod shell;
mod sloc;
mod text;
mod word;

pub use difflib::matching_blocks;
pub use float::{float_repr, round_half_even, sum_builtin};
pub use json::{JsonForm, dumps};
pub use loads::{JsonError, JsonErrorKind, JsonValue, loads};
pub use order::max_first;
pub use oserror::{os_error_repr, strerror};
pub use printable::is_printable;
pub use pystr::PyStr;
pub use shell::shlex_quote;
pub use sloc::{TokenizeError, TokenizeErrorKind, python_sloc};
pub use text::{is_space, split_whitespace, splitlines, strip};
pub use word::is_word;

#[cfg(test)]
mod fixtures {
    use serde::de::DeserializeOwned;
    use std::path::Path;

    pub(crate) fn cases<T: DeserializeOwned>(file: &str, section: &str) -> T {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(format!("{file}.json"));
        let shown = path.display();
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{shown}: {error}"));
        let mut sections: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&text).unwrap_or_else(|error| panic!("{shown}: {error}"));
        let value = sections
            .remove(section)
            .unwrap_or_else(|| panic!("{shown}: no section {section}"));
        serde_json::from_value(value).unwrap_or_else(|error| panic!("{shown}: {section}: {error}"))
    }

    // The fixtures store each float as "0x" and the 16 hex digits of its bits.
    pub(crate) fn bits(hex: &str) -> u64 {
        let digits = hex
            .strip_prefix("0x")
            .unwrap_or_else(|| panic!("{hex}: no 0x prefix"));
        u64::from_str_radix(digits, 16).unwrap_or_else(|error| panic!("{hex}: {error}"))
    }

    pub(crate) fn float(hex: &str) -> f64 {
        f64::from_bits(bits(hex))
    }

    pub(crate) fn floats(hexes: &[String]) -> Vec<f64> {
        hexes.iter().map(|hex| float(hex)).collect()
    }

    // Checks `function` on each (input, output) case of a text section.
    pub(crate) fn check_text_cases(section: &str, function: impl Fn(&str) -> String) {
        let all: Vec<(String, String)> = cases("text", section);
        for (input, expected) in all {
            assert_eq!(function(&input), expected, "{section}: input {input:?}");
        }
    }

    // Checks `inside` on every code point against the runs of code points in a text section.
    pub(crate) fn check_runs(section: &str, inside: impl Fn(u32) -> bool) {
        let runs: Vec<(u32, u32)> = cases("text", section);
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
}
