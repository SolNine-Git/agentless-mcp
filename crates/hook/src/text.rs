//! The text forms the gate's reasons share: decode and OS errors, paths, and digests.

use std::any::Any;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use pycompat::PyStr;
use sha2::{Digest, Sha256};

const TAIL_CHARS: usize = 300;

// CPython's UnicodeDecodeError text: one byte, or the span of a longer bad sequence.
pub(crate) fn codec_text(bytes: &[u8], error: io::NotUtf8) -> String {
    let io::NotUtf8 { start, end, reason } = error;
    if end == start + 1 {
        let byte = bytes[start];
        return format!(
            "'utf-8' codec can't decode byte 0x{byte:02x} in position {start}: {reason}"
        );
    }
    let last = end - 1;
    format!("'utf-8' codec can't decode bytes in position {start}-{last}: {reason}")
}

// The class and the text of an OS error, as the last line of a CPython traceback shows them.
pub(crate) fn os_error_line(error: &std::io::Error) -> String {
    let repr = pycompat::os_error_repr(error);
    let class = repr.split('(').next().unwrap_or("OSError");
    match error.raw_os_error() {
        Some(code) => format!("{class}: [Errno {code}] {}", pycompat::strerror(error)),
        None => format!("{class}: {}", pycompat::strerror(error)),
    }
}

// CPython's str() of an OS error that names a file.
pub(crate) fn os_error_text(error: &std::io::Error, path: &Path) -> String {
    let message = pycompat::strerror(error);
    match error.raw_os_error() {
        Some(code) => format!("[Errno {code}] {message}: {}", repr(path)),
        None => message,
    }
}

pub(crate) fn repr(path: &Path) -> String {
    PyStr::fsdecode(path.as_os_str().as_bytes()).repr()
}

// The last line with text, stripped and cut to its last 300 characters.
pub(crate) fn last_line(text: &str) -> String {
    let lines = pycompat::splitlines(text, false);
    let Some(line) = lines
        .iter()
        .map(|line| pycompat::strip(line))
        .rfind(|line| !line.is_empty())
    else {
        return "(no output)".to_owned();
    };
    let skip = line.chars().count().saturating_sub(TAIL_CHARS);
    line.chars().skip(skip).collect()
}

pub(crate) fn lossy(text: &PyStr) -> String {
    text.code_points()
        .map(|code| char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

pub(crate) fn shown(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// Digits in groups of three, joined by commas.
pub(crate) fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut shown = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            shown.push(',');
        }
        shown.push(digit);
    }
    shown
}

pub(crate) fn panic_text(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "(no message)".to_owned())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    kernel::Key::new(Sha256::digest(bytes).into()).to_string()
}

#[cfg(test)]
mod tests;
