use super::IntArg;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const FIXTURE: &str = "read/bounds";

#[test]
fn an_integer_text_reads_as_the_reference_reads_it() {
    let cases: Vec<(String, Option<String>)> = io::testing::cases(MANIFEST_DIR, FIXTURE, "ints");
    assert!(!cases.is_empty());
    for (text, expected) in cases {
        let read = IntArg::parse(&text).map(|value| value.to_string());
        assert_eq!(read, expected, "{text:?}");
    }
}

// Ledger deviation: the reference reads Unicode digits, and this build refuses them.
#[test]
fn a_unicode_digit_is_refused_where_the_reference_reads_it() {
    let cases: Vec<(String, Option<String>)> =
        io::testing::cases(MANIFEST_DIR, FIXTURE, "deviations");
    assert!(!cases.is_empty());
    for (text, reference) in cases {
        assert!(reference.is_some(), "{text:?}: the reference reads it");
        assert_eq!(IntArg::parse(&text), None, "{text:?}");
    }
}

#[test]
fn the_value_saturates_and_a_negative_has_none() {
    let value = |text: &str| IntArg::parse(text).and_then(|arg| arg.value());
    assert_eq!(value("42"), Some(42));
    assert_eq!(value("-0"), Some(0));
    assert_eq!(value("-1"), None);
    assert_eq!(value(&"9".repeat(40)), Some(usize::MAX));
    assert_eq!(IntArg::from_usize(7).to_string(), "7");
}
