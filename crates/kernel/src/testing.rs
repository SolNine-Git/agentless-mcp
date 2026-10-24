//! A clock for tests: a wall time fixed at construction, and a monotonic time that moves only
//! when the test says.

use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use crate::Clock;

/// A wall time fixed at construction, and a monotonic time that moves only when a test says.
#[derive(Debug)]
pub struct PinnedClock {
    wall: SystemTime,
    base: Instant,
    elapsed: Mutex<Duration>,
}

impl PinnedClock {
    /// Returns a clock whose wall time is `wall` and whose monotonic time starts now.
    #[must_use]
    pub fn at(wall: SystemTime) -> Self {
        Self {
            wall,
            base: Instant::now(),
            elapsed: Mutex::new(Duration::ZERO),
        }
    }

    /// Moves the monotonic time forward by `by`.
    ///
    /// # Panics
    ///
    /// Panics if another test thread panicked while it held the clock's lock.
    pub fn advance(&self, by: Duration) {
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
