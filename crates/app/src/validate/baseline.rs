//! The baseline: the test command on unpatched HEAD, repeated as asked, then the reproduction
//! command once. Any disagreement between the repeats makes the run unverified.

use io::{RunResult, RunStatus};

use super::budget::{self, Limits};
use super::records::{BaselineStatus, ReproVerdict, RunHeader};
use super::{Run, ValidateRequest};
use crate::{Error, Noted};

const NOT_STARTED: &str = "the aggregate run budget expired before the baseline command started";

// What the baseline runs, taken together, established.
#[derive(Clone, Debug, PartialEq)]
struct Outcome {
    status: BaselineStatus,
    detail: String,
    representative: Option<RunResult>,
    repeats: usize,
    failures: usize,
    deadline_expired: bool,
    budget_truncated: bool,
}

// The repeats that ran: all of them, or the ones before the budget expired.
enum Repeats {
    Done {
        runs: Vec<RunResult>,
        truncated: bool,
    },
    Expired(Vec<RunResult>),
}

impl Run<'_> {
    // The run header, and whether the budget abandoned work the baseline was asked for.
    pub(super) fn baseline(&self, count: usize) -> Noted<Result<(RunHeader, bool), Error>> {
        self.baseline_runs().then_ok(|repeats| match repeats {
            Repeats::Expired(runs) => {
                let outcome = expired_baseline(&runs, self.limits.repeats, &self.budget());
                Noted::new(Ok((self.header(count, outcome, None), true)))
            }
            Repeats::Done { runs, truncated } => {
                let outcome = baseline_outcome(&runs, self.limits.repeats, truncated);
                if outcome.status == BaselineStatus::Unverified {
                    return Noted::new(Ok((self.header(count, outcome, None), false)));
                }
                self.repro_baseline(count, outcome)
            }
        })
    }

    fn baseline_runs(&self) -> Noted<Result<Repeats, Error>> {
        let repeats = self.limits.repeats;
        let mut runs = Vec::new();
        let mut truncated = false;
        let mut diagnostics = Vec::new();
        while runs.len() < repeats {
            let Some(bound) = self.bound() else {
                return Noted {
                    value: Ok(Repeats::Expired(runs)),
                    diagnostics,
                };
            };
            let command = &self.request.test_cmd;
            let ran = self.in_worktree(|tree| self.run_command(tree.inside(), command, &bound));
            diagnostics.extend(ran.diagnostics);
            let run = match ran.value {
                Ok(run) => run,
                Err(error) => {
                    return Noted {
                        value: Err(error),
                        diagnostics,
                    };
                }
            };
            truncated = truncated || budget::truncated(&bound, &run);
            runs.push(run);
            if self.reached() && runs.len() < repeats {
                return Noted {
                    value: Ok(Repeats::Expired(runs)),
                    diagnostics,
                };
            }
        }
        Noted {
            value: Ok(Repeats::Done { runs, truncated }),
            diagnostics,
        }
    }

    fn repro_baseline(
        &self,
        count: usize,
        outcome: Outcome,
    ) -> Noted<Result<(RunHeader, bool), Error>> {
        let Some(command) = self.request.repro_cmd.as_deref() else {
            return Noted::new(Ok((self.header(count, outcome, None), false)));
        };
        let Some(bound) = self.bound() else {
            let expired = Outcome {
                deadline_expired: true,
                ..outcome
            };
            return Noted::new(Ok((self.header(count, expired, None), true)));
        };
        let ran = self.in_worktree(|tree| self.run_command(tree.inside(), command, &bound));
        ran.map_ok(|run| {
            let budget_truncated = outcome.budget_truncated || budget::truncated(&bound, &run);
            let outcome = Outcome {
                budget_truncated,
                ..outcome
            };
            (self.header(count, outcome, Some(run)), false)
        })
    }

    fn header(&self, count: usize, outcome: Outcome, repro_run: Option<RunResult>) -> RunHeader {
        header(self.request, &self.limits, count, outcome, repro_run)
    }
}

// An incomplete baseline is unverified, and every required repeat counts as failed.
fn expired_baseline(runs: &[RunResult], repeats: usize, budget: &str) -> Outcome {
    let started = runs.last().cloned().unwrap_or_else(|| RunResult {
        status: RunStatus::Timeout,
        exit_code: None,
        duration: 0.0,
        stdout_tail: String::new(),
        stderr_tail: NOT_STARTED.to_owned(),
    });
    Outcome {
        status: BaselineStatus::Unverified,
        detail: format!(
            "the run's {budget}s budget expired after {} of {repeats} required baseline runs",
            runs.len()
        ),
        representative: Some(started),
        repeats,
        failures: repeats,
        deadline_expired: true,
        budget_truncated: false,
    }
}

// All passed, all failed, or they disagreed: a mixed result is an instrument that reads
// differently each time, and it invalidates the run as a red baseline does.
fn baseline_outcome(runs: &[RunResult], repeats: usize, truncated: bool) -> Outcome {
    let failures: Vec<&RunResult> = runs.iter().filter(|run| !run.passed()).collect();
    let (status, detail, representative) = match failures.first() {
        None => (BaselineStatus::Ok, passed_detail(repeats), runs.last()),
        Some(first) if failures.len() < repeats => {
            let detail = format!(
                "FLAKY BASELINE: the test command disagreed across {repeats} runs on unpatched \
                 HEAD -- {} failed and {} passed, with nothing changing between them",
                failures.len(),
                repeats - failures.len()
            );
            (BaselineStatus::Unverified, detail, Some(*first))
        }
        Some(first) => {
            let detail = baseline_detail(first, truncated);
            (BaselineStatus::Unverified, detail, Some(*first))
        }
    };
    Outcome {
        status,
        detail,
        representative: representative.cloned(),
        repeats,
        failures: failures.len(),
        deadline_expired: false,
        budget_truncated: truncated,
    }
}

fn passed_detail(repeats: usize) -> String {
    if repeats == 1 {
        return "the test command passed on unpatched HEAD".to_owned();
    }
    format!("the test command passed on unpatched HEAD in all {repeats} baseline runs")
}

// A timeout names the bound that produced it, so a budget-cut baseline is not read as slow.
fn baseline_detail(run: &RunResult, truncated: bool) -> String {
    match run.status {
        RunStatus::Timeout => {
            let bound = if truncated {
                "the run's remaining budget"
            } else {
                "its own timeout"
            };
            format!(
                "the test command timed out after {}s on unpatched HEAD, bounded by {bound}",
                json::float_text(run.duration)
            )
        }
        RunStatus::Error => format!(
            "the test command could not be started: {}",
            text::strip(&run.stderr_tail)
        ),
        _ => {
            let code = run
                .exit_code
                .map_or_else(|| "None".to_owned(), |code| code.to_string());
            format!("the test command exited {code} on unpatched HEAD")
        }
    }
}

fn header(
    request: &ValidateRequest,
    limits: &Limits,
    count: usize,
    outcome: Outcome,
    repro_run: Option<RunResult>,
) -> RunHeader {
    let repro_verdict = if outcome.status == BaselineStatus::Unverified || outcome.deadline_expired
    {
        match request.repro_cmd {
            None => ReproVerdict::NotGiven,
            Some(_) => ReproVerdict::NotEvaluated,
        }
    } else {
        repro_verdict(repro_run.as_ref())
    };
    RunHeader {
        test_cmd: request.test_cmd.clone(),
        repro_cmd: request.repro_cmd.clone(),
        timeout: limits.timeout,
        jobs: limits.jobs,
        candidates: count,
        baseline: outcome.status,
        baseline_detail: outcome.detail,
        repro_verdict,
        baseline_run: outcome.representative,
        repro_baseline_run: repro_run,
        repeat_baseline: outcome.repeats,
        baseline_failures: outcome.failures,
        allow_test_config_edits: request.allow_test_config_edits,
        budget_truncated: outcome.budget_truncated,
    }
}

// A failure or a timeout is the bug showing itself; a pass does not reproduce it, and a spawn
// error means there was no test to speak of.
fn repro_verdict(run: Option<&RunResult>) -> ReproVerdict {
    match run.map(|run| run.status) {
        None => ReproVerdict::NotGiven,
        Some(RunStatus::Passed) => ReproVerdict::DoesNotReproduce,
        Some(RunStatus::Error) => ReproVerdict::Unrunnable,
        Some(_) => ReproVerdict::Reproduces,
    }
}

#[cfg(test)]
mod tests;
