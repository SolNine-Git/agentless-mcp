//! File reads with a size cap, and strict UTF-8 decoding.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::str::Utf8Error;

use crate::{byte_count, strerror};

/// The most bytes that [`read_bounded`] returns.
pub const MAX_FILE_BYTES: usize = 1_000_000;

/// Why [`read_bounded`] returned no bytes. It displays as the skip reason.
#[derive(Debug)]
pub enum Skipped {
    /// The file holds more than [`MAX_FILE_BYTES`] bytes.
    TooLarge {
        /// The largest size seen: before the read, after it, or the bytes read.
        size: u64,
    },
    /// The open, the stat or the read failed.
    Unreadable(io::Error),
}

impl fmt::Display for Skipped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { size } => write!(
                f,
                "skipped: {size} bytes exceeds the per-file cap of {MAX_FILE_BYTES} bytes"
            ),
            Self::Unreadable(error) => write!(f, "unreadable: {}", strerror(error)),
        }
    }
}

impl std::error::Error for Skipped {}

/// Reads all of `path`, following symlinks, or skips it whole when it holds more than
/// [`MAX_FILE_BYTES`] bytes.
///
/// # Errors
///
/// Returns [`Skipped`] when the file is too large or cannot be read.
pub fn read_bounded(path: &Path) -> Result<Vec<u8>, Skipped> {
    let file = File::open(path).map_err(Skipped::Unreadable)?;
    let (data, sizes) = read_capped(file).map_err(Skipped::Unreadable)?;
    if data.len() <= MAX_FILE_BYTES {
        return Ok(data);
    }
    let size = sizes.0.max(sizes.1).max(byte_count(&data));
    Err(Skipped::TooLarge { size })
}

// Both stats run on every read: one must come before the read and one after it.
fn read_capped(mut file: File) -> io::Result<(Vec<u8>, (u64, u64))> {
    let before = file.metadata()?.len();
    let limit = u64::try_from(MAX_FILE_BYTES + 1).unwrap_or(u64::MAX);
    // The spare byte lets the read that finds the end go into the buffer without a regrow.
    let room = usize::try_from(before.min(limit)).map_or(MAX_FILE_BYTES + 1, |size| size + 1);
    let mut data = Vec::with_capacity(room);
    (&mut file).take(limit).read_to_end(&mut data)?;
    let after = file.metadata()?.len();
    Ok((data, (before, after)))
}

/// Why bytes are not UTF-8 text, as the decoder names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Utf8Reason {
    /// A byte that cannot start a sequence.
    InvalidStart,
    /// A sequence whose start is valid and whose next byte is not.
    InvalidContinuation,
    /// A valid start of a sequence at the end of the bytes.
    UnexpectedEnd,
}

impl fmt::Display for Utf8Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidStart => "invalid start byte",
            Self::InvalidContinuation => "invalid continuation byte",
            Self::UnexpectedEnd => "unexpected end of data",
        })
    }
}

/// Bytes that are not UTF-8 text: the span of the first bad sequence and why it is bad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotUtf8 {
    /// The offset of the first byte of the bad sequence.
    pub start: usize,
    /// The offset just past the bad sequence. A sequence cut off by the end runs to the end.
    pub end: usize,
    /// Why the sequence is bad.
    pub reason: Utf8Reason,
}

impl fmt::Display for NotUtf8 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "not UTF-8 text: byte {} is {}", self.start, self.reason)
    }
}

impl std::error::Error for NotUtf8 {}

/// Decodes `bytes` as UTF-8 text.
///
/// # Errors
///
/// Returns [`NotUtf8`] for the first sequence that is not UTF-8.
pub fn decode_strict(bytes: Vec<u8>) -> Result<String, NotUtf8> {
    String::from_utf8(bytes).map_err(|error| not_utf8(error.as_bytes(), error.utf8_error()))
}

fn not_utf8(bytes: &[u8], error: Utf8Error) -> NotUtf8 {
    let start = error.valid_up_to();
    let reason = match (error.error_len(), bytes[start]) {
        (None, _) => Utf8Reason::UnexpectedEnd,
        (Some(_), 0xc2..=0xf4) => Utf8Reason::InvalidContinuation,
        (Some(_), _) => Utf8Reason::InvalidStart,
    };
    let end = error
        .error_len()
        .map_or(bytes.len(), |length| start + length);
    NotUtf8 { start, end, reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{build, fixture};
    use serde_json::Value;

    #[test]
    fn decode_cases() {
        let all: Vec<(Vec<u8>, Value)> = fixture("read", "decode");
        for (bytes, expected) in all {
            let found = match decode_strict(bytes.clone()) {
                Ok(text) => serde_json::json!(["text", text]),
                Err(error) => {
                    let reason = error.reason.to_string();
                    serde_json::json!(["error", error.start, reason, error.to_string()])
                }
            };
            assert_eq!(found, expected, "bytes: {bytes:x?}");
        }
    }

    // The spans CPython's codec error reports for these inputs: one byte, then two two-byte ones.
    #[test]
    fn the_end_closes_the_span_the_decoder_names() {
        let cases: [(&[u8], usize, usize, Utf8Reason); 3] = [
            (b"\xff{}", 0, 1, Utf8Reason::InvalidStart),
            (
                b"{\"x\": \"\xe2\x82\"}",
                7,
                9,
                Utf8Reason::InvalidContinuation,
            ),
            (b"\xf0\x90\x28", 0, 2, Utf8Reason::InvalidContinuation),
        ];
        for (bytes, start, end, reason) in cases {
            let refused = decode_strict(bytes.to_vec());
            assert_eq!(refused, Err(NotUtf8 { start, end, reason }), "{bytes:x?}");
        }
        let cut = decode_strict(b"ab\xe2\x82".to_vec()).map_err(|error| error.end);
        assert_eq!(cut, Err(4));
    }

    #[test]
    fn bounded_cases() {
        let all: Vec<(String, Vec<Value>, String, Value)> = fixture("read", "bounded");
        for (name, layout, path, expected) in all {
            let tree = build(&layout);
            let found = match read_bounded(&tree.join(path)) {
                Ok(data) => serde_json::json!(["bytes", data.len()]),
                Err(skipped) => serde_json::json!(["skipped", skipped.to_string()]),
            };
            assert_eq!(found, expected, "case {name}");
        }
    }
}
