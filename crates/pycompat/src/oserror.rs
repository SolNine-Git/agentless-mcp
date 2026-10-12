//! The text forms of an OS error: its message, and the repr of the exception class for its errno.

use std::io;

use crate::PyStr;

// The errno values that have an exception class of their own; every other errno is OSError.
const CLASSES: [(i32, &str); 19] = [
    (libc::EAGAIN, "BlockingIOError"),
    (libc::EALREADY, "BlockingIOError"),
    (libc::EINPROGRESS, "BlockingIOError"),
    (libc::EWOULDBLOCK, "BlockingIOError"),
    (libc::EPIPE, "BrokenPipeError"),
    (libc::ESHUTDOWN, "BrokenPipeError"),
    (libc::ECHILD, "ChildProcessError"),
    (libc::ECONNABORTED, "ConnectionAbortedError"),
    (libc::ECONNREFUSED, "ConnectionRefusedError"),
    (libc::ECONNRESET, "ConnectionResetError"),
    (libc::EEXIST, "FileExistsError"),
    (libc::ENOENT, "FileNotFoundError"),
    (libc::EISDIR, "IsADirectoryError"),
    (libc::ENOTDIR, "NotADirectoryError"),
    (libc::EINTR, "InterruptedError"),
    (libc::EACCES, "PermissionError"),
    (libc::EPERM, "PermissionError"),
    (libc::ESRCH, "ProcessLookupError"),
    (libc::ETIMEDOUT, "TimeoutError"),
];

/// Returns the message of `error` without the " (os error N)" suffix of its display.
#[must_use]
pub fn strerror(error: &io::Error) -> String {
    let text = error.to_string();
    let Some(code) = error.raw_os_error() else {
        return text;
    };
    match text.strip_suffix(&format!(" (os error {code})")) {
        Some(message) => message.to_owned(),
        None => text,
    }
}

/// Returns the repr of the exception for `error`: the class for its errno, the errno, and the
/// message, for example `PermissionError(13, 'Permission denied')`. An error without an errno
/// gives `OSError('<message>')`.
#[must_use]
pub fn os_error_repr(error: &io::Error) -> String {
    let message = PyStr::from(strerror(error).as_str()).repr();
    let Some(code) = error.raw_os_error() else {
        return format!("OSError({message})");
    };
    let class = CLASSES
        .iter()
        .find(|(errno, _)| *errno == code)
        .map_or("OSError", |(_, class)| class);
    format!("{class}({code}, {message})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn os_error_repr_cases() {
        let all: Vec<(i32, String)> = crate::fixtures::cases("os", "os_error_repr");
        for (code, expected) in all {
            let error = io::Error::from_raw_os_error(code);
            assert_eq!(os_error_repr(&error), expected, "errno {code}");
        }
    }

    #[test]
    fn each_errno_class_and_the_fallbacks() {
        let missing = io::Error::from_raw_os_error(libc::ENOENT);
        assert!(os_error_repr(&missing).starts_with("FileNotFoundError("));
        let timeout = io::Error::from_raw_os_error(libc::ETIMEDOUT);
        assert!(os_error_repr(&timeout).starts_with("TimeoutError("));
        let invalid = io::Error::from_raw_os_error(libc::EINVAL);
        assert!(os_error_repr(&invalid).starts_with("OSError("));
        let custom = io::Error::other("it's custom");
        assert_eq!(os_error_repr(&custom), "OSError(\"it's custom\")");
        assert_eq!(strerror(&custom), "it's custom");
    }
}
