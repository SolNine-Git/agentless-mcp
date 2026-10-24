//! The time port: the wall clock for ages shown to the reader, and the monotonic clock for budgets.

pub use kernel::{Clock, SystemClock};

#[cfg(test)]
pub(crate) use kernel::testing::PinnedClock;

// A deadline whose allowance ran out a minute ago, on a clock that no longer moves.
#[cfg(test)]
pub(crate) fn spent() -> kernel::Deadline {
    use std::sync::Arc;
    use std::time::{Duration, UNIX_EPOCH};

    let clock = Arc::new(PinnedClock::at(UNIX_EPOCH));
    let deadline = kernel::Deadline::new(1.0, 0.0, clock.clone());
    clock.advance(Duration::from_secs(61));
    deadline
}

// Panics unless `error` is the stop that a `spent()` deadline gives.
#[cfg(test)]
pub(crate) fn assert_spent(error: &crate::Error) {
    let stopped = kernel::Stopped::Spent {
        elapsed: 61.0,
        limit: 1.0,
        grace: 0.0,
        credited: 0.0,
    };
    let crate::Error::Kind(kernel::Error::CallStopped(message)) = error else {
        panic!("not a stopped call: {error:?}");
    };
    assert_eq!(*message, crate::Error::from(stopped).to_string());
}

#[cfg(test)]
mod tests;
