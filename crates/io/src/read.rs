//! File reads with a size cap, and strict UTF-8 decoding.

use std::fmt;
use std::fs::{self, File, Metadata};
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

/// Returns the metadata of `path` after symlinks, or `None` when nothing is there: a missing name,
/// a parent that is not a directory, a symlink loop, or a bad descriptor.
///
/// # Errors
///
/// Returns every other error of the stat, so a file that cannot be seen never reads as a missing
/// one.
pub fn stat_present(path: &Path) -> io::Result<Option<Metadata>> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if is_absent(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_absent(error: &io::Error) -> bool {
    let absent = [libc::ENOENT, libc::ENOTDIR, libc::ELOOP, libc::EBADF];
    error
        .raw_os_error()
        .is_some_and(|code| absent.contains(&code))
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

/// Decodes `bytes` as UTF-8 text, with each "\r\n" and each lone "\r" read as "\n".
///
/// # Errors
///
/// Returns [`NotUtf8`] for the first sequence that is not UTF-8.
pub fn decode_text(bytes: Vec<u8>) -> Result<String, NotUtf8> {
    let text = decode_strict(bytes)?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
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
    use crate::fixtures::{build, fixture, tree};
    use serde_json::Value;

    // What a stat shows: "file", "dir", "absent", or the unreadable reason.
    fn stat_shown(path: &Path) -> String {
        match stat_present(path) {
            Ok(Some(metadata)) if metadata.is_dir() => "dir".to_owned(),
            Ok(Some(_)) => "file".to_owned(),
            Ok(None) => "absent".to_owned(),
            Err(error) => Skipped::Unreadable(error).to_string(),
        }
    }

    #[test]
    fn only_a_missing_name_a_file_parent_and_a_loop_read_as_absent() {
        let tree = tree();
        let file = tree
            .write_file("plain.py", "x = 1\n")
            .expect("the file is written");
        tree.symlink("loop", tree.join("loop"))
            .expect("the loop is made");
        assert_eq!(stat_shown(&file), "file");
        assert_eq!(stat_shown(tree.path()), "dir");
        assert_eq!(stat_shown(&tree.join("gone.py")), "absent");
        assert_eq!(stat_shown(&file.join("under.py")), "absent");
        assert_eq!(stat_shown(&tree.join("loop")), "absent");
        let long = tree.join("n".repeat(300));
        assert_eq!(stat_shown(&long), "unreadable: File name too long");
    }

    #[test]
    fn a_file_below_an_unsearchable_directory_is_unreadable_not_absent() {
        let tree = tree();
        tree.create_dir("secret").expect("the directory is created");
        tree.write_file("secret/x.py", "x = 1\n")
            .expect("the file is written");
        if !tree.modes_bind() {
            return;
        }
        tree.chmod("secret", 0o000);
        let shown = stat_shown(&tree.join("secret/x.py"));
        tree.chmod("secret", 0o755);
        assert_eq!(shown, "unreadable: Permission denied");
    }

    #[test]
    fn a_text_decode_reads_each_carriage_return_as_a_line_break() {
        let text = decode_text(b"one\r\ntwo\rthree\n\r\n".to_vec()).expect("UTF-8 text");
        assert_eq!(text, "one\ntwo\nthree\n\n");
        let error = decode_text(b"ok\r\n\xff".to_vec()).expect_err("not UTF-8");
        assert_eq!(error.start, 4);
    }

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
