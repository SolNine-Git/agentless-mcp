//! The text forms of an OS error: its message, its class name with its errno and message, and the
//! form that names a file.

use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use text::CodePointString;

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

/// Returns the class name for the errno of `error`, the errno, and the quoted message, for
/// example `PermissionError(13, 'Permission denied')`. An error without an errno gives
/// `OSError('<message>')`.
#[must_use]
pub fn os_error_class_text(error: &io::Error) -> String {
    let message = CodePointString::from(strerror(error).as_str()).quoted();
    let Some(code) = error.raw_os_error() else {
        return format!("OSError({message})");
    };
    format!("{}({code}, {message})", class(code))
}

/// Returns the class name for the errno of `error`, the errno, and the message, for example
/// `PermissionError: [Errno 13] Permission denied`. An error without an errno gives
/// `OSError: <message>`.
#[must_use]
pub fn os_error_line(error: &io::Error) -> String {
    let message = strerror(error);
    match error.raw_os_error() {
        Some(code) => format!("{}: [Errno {code}] {message}", class(code)),
        None => format!("OSError: {message}"),
    }
}

/// Returns the text of an OS error that names the file `path`, for example
/// `[Errno 13] Permission denied: '/c/g.html'`. A byte of `path` that is not UTF-8 is quoted as a
/// surrogate escape, and an error without an errno gives its message alone.
#[must_use]
pub fn os_error_text(error: &io::Error, path: &Path) -> String {
    let message = strerror(error);
    let Some(code) = error.raw_os_error() else {
        return message;
    };
    let quoted = CodePointString::from_utf8_escaped(path.as_os_str().as_bytes()).quoted();
    format!("[Errno {code}] {message}: {quoted}")
}

fn class(code: i32) -> &'static str {
    CLASSES
        .iter()
        .find(|(errno, _)| *errno == code)
        .map_or("OSError", |(_, class)| class)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The messages are the C library's, so the tests that call this run on Linux only.
    #[cfg(target_os = "linux")]
    fn check_errno_cases(section: &str, shown: impl Fn(&io::Error) -> String) {
        let all: Vec<(i32, String)> = crate::fixtures::fixture("oserror", section);
        for (code, expected) in all {
            let error = io::Error::from_raw_os_error(code);
            assert_eq!(shown(&error), expected, "{section}: errno {code}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn os_error_class_text_cases() {
        check_errno_cases("os_error_repr", os_error_class_text);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn os_error_line_cases() {
        check_errno_cases("os_error_line", os_error_line);
    }

    #[test]
    fn an_error_that_names_a_file_quotes_it_with_surrogate_escapes() {
        let denied = io::Error::from_raw_os_error(libc::EACCES);
        let target = Path::new(std::ffi::OsStr::from_bytes(b"/c/x\xffy.html"));
        assert_eq!(
            os_error_text(&denied, target),
            r"[Errno 13] Permission denied: '/c/x\udcffy.html'"
        );
        let plain = io::Error::other("no errno");
        assert_eq!(os_error_text(&plain, target), "no errno");
    }

    #[test]
    fn an_error_without_an_errno_has_the_base_class_line() {
        let custom = io::Error::other("it's custom");
        assert_eq!(os_error_line(&custom), "OSError: it's custom");
    }

    #[test]
    fn each_errno_class_and_the_fallbacks() {
        let missing = io::Error::from_raw_os_error(libc::ENOENT);
        assert!(os_error_class_text(&missing).starts_with("FileNotFoundError("));
        let timeout = io::Error::from_raw_os_error(libc::ETIMEDOUT);
        assert!(os_error_class_text(&timeout).starts_with("TimeoutError("));
        let invalid = io::Error::from_raw_os_error(libc::EINVAL);
        assert!(os_error_class_text(&invalid).starts_with("OSError("));
        let custom = io::Error::other("it's custom");
        assert_eq!(os_error_class_text(&custom), "OSError(\"it's custom\")");
        assert_eq!(strerror(&custom), "it's custom");
    }
}
