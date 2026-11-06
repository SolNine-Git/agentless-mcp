//! A hook's job run on its own thread, so that the hook stops waiting for it at the deadline.

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use app::{LintReport, LintRequest, Repo, Services};

use crate::text::panic_text;

pub(crate) type LintFn = fn(&Services, &Repo, &LintRequest) -> Result<LintReport, app::Error>;

// The whole check of one proposed text: the cache open, then lint.
pub(crate) fn in_process(
    services: &Services,
    repo: &Repo,
    request: &LintRequest,
) -> Result<LintReport, app::Error> {
    let tags = app::Tags::open(services, repo).value?;
    app::lint(services, repo, &tags, request).value
}

// Everything the lint thread owns, so that it can outlive the wait.
pub(crate) struct Job {
    pub(crate) services: Services,
    pub(crate) repo: Repo,
    pub(crate) request: LintRequest,
}

impl Job {
    // The lint of this job as work that a thread can own.
    pub(crate) fn linted_by(
        self,
        lint: LintFn,
    ) -> impl FnOnce() -> Result<LintReport, app::Error> + Send + 'static {
        move || lint(&self.services, &self.repo, &self.request)
    }
}

#[derive(Debug)]
pub(crate) enum Finished<T> {
    Done(T),
    Failed(app::Error),
    Panicked(String),
    TimedOut,
}

struct Slot<T> {
    result: Mutex<Option<Finished<T>>>,
    filled: Condvar,
}

// Waits at most `budget`. A thread still running at the deadline runs on until the process
// exits. It only reads, apart from the cache's time stamp, so the exit leaves nothing half written.
pub(crate) fn run<T: Send + 'static>(
    name: &str,
    work: impl FnOnce() -> Result<T, app::Error> + Send + 'static,
    budget: Duration,
) -> std::io::Result<Finished<T>> {
    let slot = Arc::new(Slot {
        result: Mutex::new(None),
        filled: Condvar::new(),
    });
    let filled = Arc::clone(&slot);
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let finished = match panic::catch_unwind(AssertUnwindSafe(work)) {
                Ok(Ok(value)) => Finished::Done(value),
                Ok(Err(error)) => Finished::Failed(error),
                Err(payload) => Finished::Panicked(panic_text(&*payload)),
            };
            let mut result = filled.result.lock().unwrap_or_else(PoisonError::into_inner);
            *result = Some(finished);
            filled.filled.notify_one();
        })?;
    let result = slot.result.lock().unwrap_or_else(PoisonError::into_inner);
    let (mut result, _) = slot
        .filled
        .wait_timeout_while(result, budget, |result| result.is_none())
        .unwrap_or_else(PoisonError::into_inner);
    Ok(result.take().unwrap_or(Finished::TimedOut))
}

#[cfg(test)]
mod tests;
