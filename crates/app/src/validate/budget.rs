//! The request's numbers, checked once, and the time each command may take: its own timeout, or
//! what is left of the run's budget.

use std::time::{Duration, Instant};

use io::{RunResult, RunStatus};

use super::{MAX_JOBS, ValidateRequest};
use crate::{Error, bounds};

// Below this a command measures the budget rather than the suite, so it is not run at all.
pub(crate) const MIN_COMMAND: Duration = Duration::from_secs(1);

// The request's numbers after the one check, saturated at usize::MAX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    pub(crate) timeout: usize,
    pub(crate) jobs: usize,
    pub(crate) repeats: usize,
    pub(crate) budget: Option<usize>,
}

// Refused rather than clamped: a bound the tool rewrites is a bound the caller did not set.
pub(crate) fn checked(request: &ValidateRequest) -> Result<Limits, Error> {
    let jobs = bounds::within(&request.jobs, 1, MAX_JOBS, "jobs")?;
    let timeout = bounds::at_least(&request.timeout, 1, "timeout")?;
    let repeats = bounds::at_least(&request.repeat_baseline, 1, "repeat_baseline")?;
    let budget = request.run_timeout.as_ref();
    let budget = budget
        .map(|seconds| bounds::at_least(seconds, 1, "run_timeout"))
        .transpose()?;
    Ok(Limits {
        timeout,
        jobs,
        repeats,
        budget,
    })
}

// The instant by which the whole run must finish. A budget past the clock's range is none.
pub(crate) fn deadline(now: Instant, budget: Option<usize>) -> Option<Instant> {
    now.checked_add(seconds(budget?))
}

pub(crate) fn reached(deadline: Option<Instant>, now: Instant) -> bool {
    deadline.is_some_and(|at| now >= at)
}

// How long one command may run, and whether the budget rather than its own timeout decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bound {
    pub(crate) time: Duration,
    pub(crate) from_budget: bool,
}

// None when the budget left is below MIN_COMMAND: no time is the honest answer then.
pub(crate) fn command_bound(
    timeout: usize,
    deadline: Option<Instant>,
    now: Instant,
) -> Option<Bound> {
    let own = Bound {
        time: seconds(timeout),
        from_budget: false,
    };
    let Some(at) = deadline else {
        return Some(own);
    };
    let remaining = at.saturating_duration_since(now);
    if remaining < MIN_COMMAND {
        return None;
    }
    if remaining < own.time {
        return Some(Bound {
            time: remaining,
            from_budget: true,
        });
    }
    Some(own)
}

// True when the budget, not the caller's own timeout, ended the command.
pub(crate) fn truncated(bound: &Bound, run: &RunResult) -> bool {
    bound.from_budget && run.status == RunStatus::Timeout
}

fn seconds(count: usize) -> Duration {
    Duration::from_secs(u64::try_from(count).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests;
