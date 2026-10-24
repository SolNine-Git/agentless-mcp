//! The lint run on its own thread, so that the gate stops waiting for it at the deadline.

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

// Everything the thread owns, so that it can outlive the wait.
pub(crate) struct Job {
    pub(crate) services: Services,
    pub(crate) repo: Repo,
    pub(crate) request: LintRequest,
}

#[derive(Debug)]
pub(crate) enum Linted {
    Report(LintReport),
    Failed(app::Error),
    Panicked(String),
    TimedOut,
}

#[derive(Default)]
struct Slot {
    result: Mutex<Option<Linted>>,
    filled: Condvar,
}

// Waits at most `budget`. A thread still running at the deadline runs on until the process
// exits. It only reads, apart from the cache's time stamp, so the exit leaves nothing half written.
pub(crate) fn run(lint: LintFn, job: Job, budget: Duration) -> std::io::Result<Linted> {
    let slot = Arc::new(Slot::default());
    let filled = Arc::clone(&slot);
    thread::Builder::new()
        .name("erosion-lint".to_owned())
        .spawn(move || {
            let ran = panic::catch_unwind(AssertUnwindSafe(|| {
                lint(&job.services, &job.repo, &job.request)
            }));
            let linted = match ran {
                Ok(Ok(report)) => Linted::Report(report),
                Ok(Err(error)) => Linted::Failed(error),
                Err(payload) => Linted::Panicked(panic_text(&*payload)),
            };
            let mut result = filled.result.lock().unwrap_or_else(PoisonError::into_inner);
            *result = Some(linted);
            filled.filled.notify_one();
        })?;
    let result = slot.result.lock().unwrap_or_else(PoisonError::into_inner);
    let (mut result, _) = slot
        .filled
        .wait_timeout_while(result, budget, |result| result.is_none())
        .unwrap_or_else(PoisonError::into_inner);
    Ok(result.take().unwrap_or(Linted::TimedOut))
}

#[cfg(test)]
mod tests;
