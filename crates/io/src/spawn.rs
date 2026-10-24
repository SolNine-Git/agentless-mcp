//! Scoped helper threads that fall back to the calling thread when the OS refuses one.

use std::panic;
use std::thread::{self, Builder, Scope, ScopedJoinHandle};

/// Work on a scoped helper thread, or its result when the calling thread ran it.
#[derive(Debug)]
pub struct Started<'scope, T>(State<'scope, T>);

#[derive(Debug)]
enum State<'scope, T> {
    Running(ScopedJoinHandle<'scope, T>),
    Done(T),
}

/// Starts `work` on a scoped thread that takes the caller's name, or runs it at once on the
/// calling thread when the OS refuses the thread.
pub fn spawn_or_run<'scope, T, F>(
    scope: &'scope Scope<'scope, '_>,
    work: &'scope F,
) -> Started<'scope, T>
where
    F: Fn() -> T + Sync,
    T: Send + 'scope,
{
    started(helper_builder(), scope, work)
}

/// Starts `work` as [`spawn_or_run`] does, on a thread that `builder` makes. A test passes a
/// builder that the OS refuses, to reach the fallback.
pub fn spawn_or_run_with<'scope, T, F>(
    builder: Builder,
    scope: &'scope Scope<'scope, '_>,
    work: &'scope F,
) -> Started<'scope, T>
where
    F: Fn() -> T + Sync,
    T: Send + 'scope,
{
    started(builder, scope, work)
}

impl<T> Started<'_, T> {
    /// Returns the result of the work. A panic on the helper thread resumes here with its payload.
    #[must_use]
    pub fn joined(self) -> T {
        match self.0 {
            State::Running(handle) => handle
                .join()
                .unwrap_or_else(|payload| panic::resume_unwind(payload)),
            State::Done(value) => value,
        }
    }
}

/// Returns the builder of a helper thread: it takes the caller's thread name, so a panic message
/// names the thread it named before.
pub fn helper_builder() -> Builder {
    let builder = Builder::new();
    match thread::current().name() {
        Some(name) => builder.name(name.to_owned()),
        None => builder,
    }
}

// A refused thread costs only the parallelism: the output stays what a serial run gives.
fn started<'scope, T, F>(
    builder: Builder,
    scope: &'scope Scope<'scope, '_>,
    work: &'scope F,
) -> Started<'scope, T>
where
    F: Fn() -> T + Sync,
    T: Send + 'scope,
{
    let state = match builder.spawn_scoped(scope, work) {
        Ok(handle) => State::Running(handle),
        Err(_refused) => State::Done(work()),
    };
    Started(state)
}

#[cfg(test)]
mod tests {
    use std::panic::{self, AssertUnwindSafe};
    use std::thread::{self, Builder};

    use super::{spawn_or_run, started};

    // No address space holds a stack this large, so the OS refuses the thread.
    const REFUSED_STACK: usize = 1 << 60;

    #[test]
    fn a_refused_thread_runs_the_work_on_the_calling_thread() {
        let caller = thread::current().id();
        let work = || thread::current().id();

        let ran = thread::scope(|scope| {
            let refused = Builder::new().stack_size(REFUSED_STACK);
            started(refused, scope, &work).joined()
        });

        assert_eq!(ran, caller);
    }

    #[test]
    fn a_helper_takes_the_caller_name() {
        let caller = Builder::new().name("the-caller".to_owned());
        let (helper, caller_id) = caller
            .spawn(|| {
                let work = || {
                    (
                        thread::current().id(),
                        thread::current().name().map(str::to_owned),
                    )
                };
                let found = thread::scope(|scope| spawn_or_run(scope, &work).joined());
                (found, thread::current().id())
            })
            .expect("the caller starts")
            .join()
            .expect("the caller ends");

        let (helper_id, name) = helper;
        assert_ne!(helper_id, caller_id);
        assert_eq!(name.as_deref(), Some("the-caller"));
    }

    #[test]
    fn a_panic_on_the_helper_reaches_the_caller_with_its_payload() {
        let work = || -> u8 { panic!("the helper broke") };

        let caught = panic::catch_unwind(AssertUnwindSafe(|| {
            thread::scope(|scope| spawn_or_run(scope, &work).joined())
        }));

        let payload = caught.expect_err("the panic reaches the caller");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"the helper broke"));
    }
}
