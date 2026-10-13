use super::{at_least, within, within_float};
use crate::{Error, IntArg};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const FIXTURE: &str = "read/bounds";

fn refusal<T>(outcome: Result<T, Error>) -> Option<String> {
    outcome.err().map(|error| match error {
        Error::Kind(kernel::Error::OperationFailed(message)) => message,
        other => panic!("not an operation failure: {other}"),
    })
}

fn arg(text: &str) -> IntArg {
    IntArg::parse(text).unwrap_or_else(|| panic!("{text:?} is an integer"))
}

#[test]
fn within_refuses_as_the_reference_refuses() {
    let cases: Vec<(String, usize, usize, String, Option<String>)> =
        io::testing::cases(MANIFEST_DIR, FIXTURE, "within");
    assert!(!cases.is_empty());
    for (value, low, high, name, expected) in cases {
        let outcome = within(&arg(&value), low, high, &name);
        assert_eq!(refusal(outcome), expected, "{value} in {low}..={high}");
    }
}

#[test]
fn at_least_refuses_as_the_reference_refuses() {
    let cases: Vec<(String, usize, String, Option<String>)> =
        io::testing::cases(MANIFEST_DIR, FIXTURE, "at_least");
    assert!(!cases.is_empty());
    for (value, low, name, expected) in cases {
        let outcome = at_least(&arg(&value), low, &name);
        assert_eq!(refusal(outcome), expected, "{value} >= {low}");
    }
}

#[test]
fn a_float_is_refused_as_the_reference_refuses() {
    let cases: Vec<(String, Option<String>)> = io::testing::cases(MANIFEST_DIR, FIXTURE, "floats");
    assert!(!cases.is_empty());
    for (bits, expected) in cases {
        let value = text::testing::float(&bits);
        let outcome = within_float(value, 0.0, 100.0, "resolution");
        assert_eq!(refusal(outcome), expected, "{bits}");
    }
}

#[test]
fn an_accepted_value_comes_back_saturated() {
    let huge = arg(&"9".repeat(30));
    assert_eq!(at_least(&huge, 1, "members").ok(), Some(usize::MAX));
    assert_eq!(within(&arg("7"), 1, 500, "limit").ok(), Some(7));
}
