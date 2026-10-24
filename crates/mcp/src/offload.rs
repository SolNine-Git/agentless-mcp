//! Where blocking work runs: one named thread per job, whose start can fail.

use std::io;
use std::thread;

// The work of one call, or one background index. A call's job owns the call's permit and sends
// its answer when it ends.
pub(crate) type Job = Box<dyn FnOnce() + Send + 'static>;

pub(crate) trait Offload: Send + Sync {
    // Starts `job` off the async runtime, or returns why it could not start.
    fn start(&self, job: Job) -> io::Result<()>;
}

// A new thread with this name for each job: the operating system can refuse one, and then the
// caller answers so.
pub(crate) struct Threads(pub(crate) &'static str);

impl Offload for Threads {
    fn start(&self, job: Job) -> io::Result<()> {
        thread::Builder::new()
            .name(self.0.to_owned())
            .spawn(job)
            .map(drop)
    }
}
