//! The time port: the wall clock for ages shown to the reader, and the monotonic clock for budgets.

use std::time::{Instant, SystemTime};

/// The two clocks an operation reads.
pub trait Clock: Send + Sync {
    /// Returns the wall time.
    fn wall(&self) -> SystemTime;
    /// Returns the monotonic time.
    fn now(&self) -> Instant;
}

/// The operating system's clocks.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn wall(&self) -> SystemTime {
        SystemTime::now()
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}
