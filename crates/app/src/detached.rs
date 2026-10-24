//! A value freed on a detached thread, so that an answer does not wait for the free.

use std::thread::{Builder, JoinHandle};

// What `answer` gives over `value`. Then `value` is freed on a detached thread.
pub(crate) fn answered_over<V: Send + 'static, T>(value: V, answer: impl FnOnce(&V) -> T) -> T {
    let answered = answer(&value);
    drop_detached(value);
    answered
}

// The caller gives `value` up and goes on at once.
pub(crate) fn drop_detached<T: Send + 'static>(value: T) {
    drop(dropped_on(Builder::new(), value));
}

// `None` when the OS refuses the thread: `spawn` then drops its closure, and `value` with it, on
// the calling thread. A panic in the drop stays on the helper thread.
fn dropped_on<T: Send + 'static>(builder: Builder, value: T) -> Option<JoinHandle<()>> {
    builder.spawn(move || drop(value)).ok()
}

#[cfg(test)]
mod tests;
