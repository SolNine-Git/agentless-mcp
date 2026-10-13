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

#[cfg(test)]
pub(crate) use pinned::PinnedClock;

#[cfg(test)]
mod pinned {
    use std::sync::Mutex;
    use std::time::{Duration, Instant, SystemTime};

    use super::Clock;

    // A wall time fixed at construction, and a monotonic time that moves only when a test says.
    pub(crate) struct PinnedClock {
        wall: SystemTime,
        base: Instant,
        elapsed: Mutex<Duration>,
    }

    impl PinnedClock {
        pub(crate) fn at(wall: SystemTime) -> Self {
            Self {
                wall,
                base: Instant::now(),
                elapsed: Mutex::new(Duration::ZERO),
            }
        }

        pub(crate) fn advance(&self, by: Duration) {
            let mut elapsed = self
                .elapsed
                .lock()
                .expect("no test panics while it holds the lock");
            *elapsed += by;
        }
    }

    impl Clock for PinnedClock {
        fn wall(&self) -> SystemTime {
            self.wall
        }

        fn now(&self) -> Instant {
            let elapsed = self
                .elapsed
                .lock()
                .expect("no test panics while it holds the lock");
            self.base + *elapsed
        }
    }
}

#[cfg(test)]
mod tests;
