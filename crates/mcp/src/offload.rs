//! Where a call's blocking work runs: one thread per call, whose start can fail.

use std::io;
use std::thread;

// The work of one call. It owns the call's permit and sends its answer when it ends.
pub(crate) type Job = Box<dyn FnOnce() + Send + 'static>;

pub(crate) trait Offload: Send + Sync {
    // Starts `job` off the async runtime, or returns why it could not start.
    fn start(&self, job: Job) -> io::Result<()>;
}

// A new thread for each call: the operating system can refuse one, and then the call answers so.
pub(crate) struct Threads;

impl Offload for Threads {
    fn start(&self, job: Job) -> io::Result<()> {
        thread::Builder::new()
            .name("mcp-call".to_owned())
            .spawn(job)
            .map(drop)
    }
}
