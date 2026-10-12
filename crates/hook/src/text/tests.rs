use io::testing::cases;
use serde::de::DeserializeOwned;
use text::CodePointString;

use super::{codec_text, grouped, last_line, lossy, sha256_hex};
use crate::record::seconds;
use crate::testing::{MANIFEST_DIR, latin};

fn text_cases<T: DeserializeOwned>(section: &str) -> T {
    cases(MANIFEST_DIR, "text", section)
}

#[test]
fn the_codec_text_matches_the_reference_cases() {
    let all: Vec<(String, String)> = text_cases("codec");
    assert!(!all.is_empty());
    for (raw, expected) in all {
        let bytes = latin(&raw);
        let error = io::decode_strict(bytes.clone()).expect_err("each case is not UTF-8");
        assert_eq!(codec_text(&bytes, error), expected, "bytes {bytes:x?}");
    }
}

// The reference errno numbers and messages are the Linux C library's, so these two tests run on
// Linux only.
#[cfg(target_os = "linux")]
#[test]
fn an_os_error_with_a_file_matches_the_reference_cases() {
    let all: Vec<(i32, String, String)> = text_cases("os_error");
    assert!(!all.is_empty());
    for (errno, path, expected) in all {
        let error = std::io::Error::from_raw_os_error(errno);
        assert_eq!(
            super::os_error_text(&error, std::path::Path::new(&path)),
            expected,
            "errno {errno}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn an_os_error_line_names_the_class_and_the_errno() {
    let all: Vec<(i32, String)> = text_cases("os_error_line");
    assert!(!all.is_empty());
    for (errno, expected) in all {
        let error = std::io::Error::from_raw_os_error(errno);
        assert_eq!(io::os_error_line(&error), expected, "errno {errno}");
    }
}

#[test]
fn the_last_line_matches_the_reference_cases() {
    let all: Vec<(String, String)> = text_cases("last_line");
    assert!(!all.is_empty());
    for (text, expected) in all {
        assert_eq!(last_line(&text), expected, "text {text:?}");
    }
}

#[test]
fn grouped_digits_match_the_reference_cases() {
    let all: Vec<(usize, String)> = text_cases("grouped");
    assert!(!all.is_empty());
    for (count, expected) in all {
        assert_eq!(grouped(count), expected);
    }
}

#[test]
fn the_time_stamp_matches_the_reference_cases() {
    let all: Vec<(String, String)> = text_cases("seconds");
    assert!(!all.is_empty());
    for (nanos, expected) in all {
        let nanos: u64 = nanos.parse().expect("a count of nanoseconds");
        let wall = std::time::UNIX_EPOCH + std::time::Duration::from_nanos(nanos);
        assert_eq!(json::float_text(seconds(wall)), expected, "{nanos} ns");
    }
}

#[test]
fn the_digest_matches_the_reference_cases() {
    let all: Vec<(String, String)> = text_cases("sha256");
    assert!(!all.is_empty());
    for (text, expected) in all {
        assert_eq!(sha256_hex(text.as_bytes()), expected, "text {text:?}");
    }
}

#[test]
fn a_lone_surrogate_shows_as_the_replacement_character() {
    let text = CodePointString::from_utf8_escaped(b"a\xffb");
    assert_eq!(lossy(&text), "a\u{fffd}b");
}
