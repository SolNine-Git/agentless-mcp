//! A bounded pool of scoped threads that maps a slice and keeps the input order.

use std::num::NonZeroUsize;
use std::panic;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, Scope, ScopedJoinHandle};

const MAX_THREADS: usize = 8;

// The pool size for this machine: the available parallelism, from 1 to 8.
pub(crate) fn threads() -> usize {
    thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .min(MAX_THREADS)
}

// The caller works too, so `threads` counts it. A panic in `f` reaches the caller with its own
// payload, as it does in a serial map.
pub(crate) fn map_in_order<T: Sync, R: Send>(
    items: &[T],
    threads: usize,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let workers = threads.clamp(1, items.len().max(1));
    if workers == 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let work = || claimed(items, &next, &f);
    let mut indexed: Vec<(usize, R)> = thread::scope(|scope| {
        let helpers: Vec<_> = (1..workers).filter_map(|_| helper(scope, &work)).collect();
        let mut done = work();
        for helper in helpers {
            let found = helper.join();
            done.extend(found.unwrap_or_else(|payload| panic::resume_unwind(payload)));
        }
        done
    });
    indexed.sort_unstable_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, result)| result).collect()
}

// A helper takes the caller's thread name, so a panic message names the thread it named before.
// A helper that cannot start leaves its share to the threads that did.
fn helper<'scope, W, R>(
    scope: &'scope Scope<'scope, '_>,
    work: &'scope W,
) -> Option<ScopedJoinHandle<'scope, Vec<(usize, R)>>>
where
    W: Fn() -> Vec<(usize, R)> + Sync,
    R: Send + 'scope,
{
    let mut builder = thread::Builder::new();
    if let Some(name) = thread::current().name() {
        builder = builder.name(name.to_owned());
    }
    builder.spawn_scoped(scope, work).ok()
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
