//! The file tests of the search gate. A path from a payload holds code points, and each test
//! stats it the way the reference's pathlib does.

use std::ffi::OsString;
use std::fs::Metadata;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use text::CodePointString;

use super::Suppressed;

// The lone surrogates that the file system encoding turns back into one byte each.
const ESCAPED_BYTES: std::ops::RangeInclusive<u32> = 0xdc80..=0xdcff;
const ESCAPE_BASE: u32 = 0xdc00;

// The directory that a relative path resolves against.
pub(crate) enum Base<'a> {
    // The payload's cwd, as code points.
    Text(&'a CodePointString),
    // The start-up directory.
    Path(&'a Path),
}

pub(crate) fn exists(path: &Path) -> Result<bool, Suppressed> {
    Ok(present(path)?.is_some())
}

// The stat of `path` below `base`, followed through symlinks. A path that has no bytes is no
// file: the reference's stat raises a ValueError, which its file tests answer with false.
pub(crate) fn found(
    base: &Base<'_>,
    path: &CodePointString,
) -> Result<Option<Metadata>, Suppressed> {
    match joined(base, path) {
        Some(joined) => present(&joined),
        None => Ok(None),
    }
}

fn joined(base: &Base<'_>, path: &CodePointString) -> Option<PathBuf> {
    let path = os_path(path)?;
    match base {
        Base::Text(cwd) => Some(os_path(cwd)?.join(path)),
        Base::Path(cwd) => Some(cwd.join(path)),
    }
}

// UTF-8, with each of U+DC80 to U+DCFF as the byte it escapes. Any other lone surrogate gives None.
fn os_path(text: &CodePointString) -> Option<PathBuf> {
    let mut bytes = Vec::new();
    for code in text.code_points() {
        match char::from_u32(code) {
            Some(c) => bytes.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            None if ESCAPED_BYTES.contains(&code) => bytes.push(escaped_byte(code)?),
            None => return None,
        }
    }
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

fn escaped_byte(code: u32) -> Option<u8> {
    u8::try_from(code - ESCAPE_BASE).ok()
}

// The stat of the path as pathlib normalizes it, with no "." parts and no trailing slash. A NUL
// is no file; io::stat_present raises every error that pathlib's file tests do not ignore.
fn present(path: &Path) -> Result<Option<Metadata>, Suppressed> {
    if path.as_os_str().as_bytes().contains(&0) {
        return Ok(None);
    }
    let normal: PathBuf = path.components().collect();
    io::stat_present(&normal).map_err(|_| Suppressed)
}
