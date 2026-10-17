//! The inputs that the caller names: a file read as given, its path as the messages show it, and
//! the patch text on stdin.

use std::io::Read;
use std::path::{Path, PathBuf};

use app::Services;
use text::PosixPath;

// The text of the file at `path`, or the refusal "cannot read {shown}: {reason}". The file is read
// as given: no size cap and no symlink refusal, UTF-8 only, with "\r\n" and a lone "\r" as "\n".
pub(crate) fn named_file(path: &Path, shown: &str, services: &Services) -> Result<String, String> {
    let refused = |reason: String| format!("cannot read {shown}: {reason}");
    let bytes =
        std::fs::read(below_cwd(path, services)).map_err(|error| refused(io::strerror(&error)))?;
    let text = io::decode_strict(bytes).map_err(|error| refused(not_utf8(&error)))?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

// The bytes on stdin as text, kept as given, or the refusal "cannot read stdin: {reason}".
pub(crate) fn stdin_text(stdin: &mut dyn Read) -> Result<String, String> {
    let refused = |reason: String| format!("cannot read stdin: {reason}");
    let mut bytes = Vec::new();
    stdin
        .read_to_end(&mut bytes)
        .map_err(|error| refused(io::strerror(&error)))?;
    io::decode_strict(bytes).map_err(|error| refused(not_utf8(&error)))
}

// A relative path names a place below the start-up directory, when that directory is known.
pub(crate) fn below_cwd(path: &Path, services: &Services) -> PathBuf {
    match &services.cwd {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

// The path as a pure path spells it, which is how the messages name a caller's file.
pub(crate) fn shown_path(text: &str) -> String {
    PosixPath::parse(text).render()
}

fn not_utf8(error: &io::NotUtf8) -> String {
    format!("not valid UTF-8: byte {} is {}", error.start, error.reason)
}
