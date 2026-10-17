//! The file size limit as an error: with SIGXFSZ caught, a write past the limit fails with EFBIG,
//! and the caller's cleanup and message run.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use signal_hook::consts::SIGXFSZ;

/// Catches SIGXFSZ for the life of the process, so a write past the file size limit fails with
/// "File too large" and does not end the process. A program that the process starts gets the
/// default action back at its exec. Call it before any thread starts.
///
/// # Panics
///
/// Panics when the system refuses a handler for SIGXFSZ, which it does only for a signal that
/// cannot be caught.
pub fn catch_file_size_signal() {
    let caught = signal_hook::flag::register(SIGXFSZ, Arc::new(AtomicBool::new(false)));
    caught.expect("SIGXFSZ is a signal that a process can catch");
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;

    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

    use super::*;
    use crate::fixtures::{is_child, run_child, tree};
    use crate::{strerror, write_file};

    const CAUGHT: &str = "filesize::tests::child_writes_past_its_limit_with_the_signal_caught";
    const UNCAUGHT: &str = "filesize::tests::child_writes_past_its_limit_with_the_default_action";

    #[test]
    fn a_write_past_the_limit_is_an_error_only_when_the_signal_is_caught() {
        let (status, stderr) = run_child(CAUGHT);
        assert_eq!(status.code(), Some(0), "{status:?} {stderr}");
        assert!(
            stderr.contains("the write failed: File too large"),
            "{stderr}"
        );
        let (status, stderr) = run_child(UNCAUGHT);
        assert_eq!(status.signal(), Some(SIGXFSZ), "{status:?} {stderr}");
    }

    #[test]
    fn child_writes_past_its_limit_with_the_signal_caught() {
        if is_child(CAUGHT) {
            catch_file_size_signal();
            write_past_the_limit();
        }
    }

    #[test]
    fn child_writes_past_its_limit_with_the_default_action() {
        if is_child(UNCAUGHT) {
            write_past_the_limit();
        }
    }

    // Lowers the soft limits on file size and core size, then writes 4 KiB past a 1 KiB limit.
    fn write_past_the_limit() {
        let tree = tree();
        for (resource, current) in [(Resource::Fsize, 1024), (Resource::Core, 0)] {
            let limit = Rlimit {
                current: Some(current),
                ..getrlimit(resource)
            };
            setrlimit(resource, limit).expect("a soft limit can be lowered");
        }
        let written = write_file(&tree.join("big"), &[b'x'; 4096]);
        let error = written.expect_err("the limit refuses the write");
        eprintln!("the write failed: {}", strerror(&error));
    }
}
