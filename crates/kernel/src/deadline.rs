//! One call's time limit, which the long loops check because a thread cannot be stopped from
//! outside. Parse time of files the cache does not hold is credited back, up to the grace.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::Clock;

/// One call's limit and grace in seconds, the parse time credited to it, and whether its client
/// left.
pub struct Deadline {
    limit: f64,
    grace: f64,
    started: Instant,
    // The credited seconds as f64 bits, so helper threads credit without a lock.
    credited: AtomicU64,
    cancelled: AtomicBool,
    clock: Arc<dyn Clock>,
}

/// Why a check stopped a call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stopped {
    /// The client cancelled the call.
    Cancelled,
    /// The call worked past its limit plus the grace credited to it.
    Spent {
        /// The seconds since the call started.
        elapsed: f64,
        /// The seconds of work the call is allowed.
        limit: f64,
        /// The most parse time that can be credited, in seconds.
        grace: f64,
        /// The parse time credited to the call, in seconds.
        credited: f64,
    },
}

impl Deadline {
    /// Starts a call's deadline now, on `clock`.
    #[must_use]
    pub fn new(limit: f64, grace: f64, clock: Arc<dyn Clock>) -> Deadline {
        Deadline {
            limit,
            grace,
            started: clock.now(),
            credited: AtomicU64::new(0.0_f64.to_bits()),
            cancelled: AtomicBool::new(false),
            clock,
        }
    }

    /// Marks the call abandoned, so every thread of it stops at its next check.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Adds `seconds` of parse time to the call's allowance, up to the grace.
    pub fn credit(&self, seconds: f64) {
        let mut current = self.credited.load(Ordering::Acquire);
        loop {
            let next = credited_after(f64::from_bits(current), seconds, self.grace).to_bits();
            let swapped = self.credited.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            match swapped {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }

    /// Returns the parse time credited to the call, in seconds.
    #[must_use]
    pub fn credited(&self) -> f64 {
        f64::from_bits(self.credited.load(Ordering::Acquire))
    }

    /// Returns the seconds left in the call's allowance, negative once it is spent.
    #[must_use]
    pub fn remaining(&self) -> f64 {
        self.limit + self.credited() - self.elapsed()
    }

    /// Stops the call when its client left or its allowance is spent.
    ///
    /// # Errors
    ///
    /// Returns [`Stopped::Cancelled`] after [`Deadline::cancel`], else [`Stopped::Spent`] once
    /// the elapsed time is past the limit plus the credited grace.
    pub fn check(&self) -> Result<(), Stopped> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(Stopped::Cancelled);
        }
        let elapsed = self.elapsed();
        let credited = self.credited();
        if elapsed > self.limit + credited {
            return Err(Stopped::Spent {
                elapsed,
                limit: self.limit,
                grace: self.grace,
                credited,
            });
        }
        Ok(())
    }

    fn elapsed(&self) -> f64 {
        seconds_since(&*self.clock, self.started)
    }
}

impl fmt::Debug for Deadline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Deadline")
            .field("limit", &self.limit)
            .field("grace", &self.grace)
            .field("credited", &self.credited())
            .field("cancelled", &self.cancelled.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

/// Stops the call when `deadline` says so. With no deadline, it never stops.
///
/// # Errors
///
/// Returns the [`Stopped`] of [`Deadline::check`].
pub fn checkpoint(deadline: Option<&Deadline>) -> Result<(), Stopped> {
    deadline.map_or(Ok(()), Deadline::check)
}

/// Runs `work` and credits the time it took to `deadline`, up to the grace.
pub fn recaching<T>(deadline: Option<&Deadline>, work: impl FnOnce() -> T) -> T {
    let Some(deadline) = deadline else {
        return work();
    };
    let started = deadline.clock.now();
    let value = work();
    deadline.credit(seconds_since(&*deadline.clock, started));
    value
}

/// Returns a long walk's budget: the lesser of `cap` and half the call's `remaining` seconds, and
/// never below zero. With no call deadline, it is `cap`.
#[must_use]
pub fn walk_budget(cap: Duration, remaining: Option<f64>) -> Duration {
    let Some(remaining) = remaining else {
        return cap;
    };
    let cap_seconds = cap.as_secs_f64();
    let half = remaining / 2.0;
    // Each comparison keeps its first value unless the second is strictly smaller or larger, so
    // a NaN remainder gives the cap.
    let lesser = if half < cap_seconds {
        half
    } else {
        cap_seconds
    };
    let budget = if lesser > 0.0 { lesser } else { 0.0 };
    Duration::from_secs_f64(budget)
}

// The grace stays unless the sum is strictly less, so a NaN grace stays NaN.
fn credited_after(credited: f64, seconds: f64, grace: f64) -> f64 {
    let sum = credited + seconds;
    if sum < grace { sum } else { grace }
}

fn seconds_since(clock: &dyn Clock, started: Instant) -> f64 {
    clock.now().saturating_duration_since(started).as_secs_f64()
}

#[cfg(test)]
mod tests;
