//! A bounded pool of scoped threads that maps a slice and keeps the input order.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

const MAX_THREADS: usize = 8;

// The pool size for this machine: the available parallelism, from 1 to 8.
pub(crate) fn threads() -> usize {
    thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .min(MAX_THREADS)
}

// The caller works too, so `threads` counts it. A helper that cannot start leaves its share to
// the caller. A panic in `f` reaches the caller with its own payload, as it does in a serial map.
pub(crate) fn map_in_order<T: Sync, R: Send>(
    items: &[T],
    threads: usize,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    map_with(items, threads, f, io::helper_builder)
}

// `builder` makes each helper thread: a test passes one that the OS refuses.
fn map_with<T: Sync, R: Send>(
    items: &[T],
    threads: usize,
    f: impl Fn(&T) -> R + Sync,
    builder: fn() -> thread::Builder,
) -> Vec<R> {
    let workers = threads.clamp(1, items.len().max(1));
    if workers == 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let work = || claimed(items, &next, &f);
    let mut indexed: Vec<(usize, R)> = thread::scope(|scope| {
        let helpers: Vec<_> = (1..workers)
            .map(|_| io::spawn_or_run_with(builder(), scope, &work))
            .collect();
        let mut done = work();
        for helper in helpers {
            done.extend(helper.joined());
        }
        done
    });
    indexed.sort_unstable_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, result)| result).collect()
}

// Each worker takes the next unclaimed index, so one slow item does not hold up a fixed share.
fn claimed<T, R>(items: &[T], next: &AtomicUsize, f: &impl Fn(&T) -> R) -> Vec<(usize, R)> {
    let mut done = Vec::new();
    loop {
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some(item) = items.get(index) else {
            return done;
        };
        done.push((index, f(item)));
    }
}

#[cfg(test)]
mod tests;
