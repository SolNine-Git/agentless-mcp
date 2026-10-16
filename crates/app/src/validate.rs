//! Candidate validation: the baseline first, then one bounded run per distinct candidate result.
//!
//! The test command runs on unpatched HEAD in fresh worktrees before any candidate is touched.
//! A baseline that does not pass makes the whole run unverified, and every candidate is then not
//! evaluated. Each candidate is normalized in one planning worktree; candidates whose changed
//! files are byte-identical share one execution, in one worktree per representative. The
//! command comes from the invocation: a command that the repository named is refused unless
//! the caller opted in, and a candidate that edits a file naming what the tests run is refused
//! unless the caller opted in.

mod baseline;
mod budget;
mod built;
mod candidates;
mod evaluate;
mod plan;
mod records;
mod runner;
mod warnings;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use git::Worktree;
use io::{RunResult, TestCommand, TestRun, TestRunError};

use crate::{Error, IntArg, Noted, Repo, Services};
use budget::{Bound, Limits};
use built::{deadline_expired, not_evaluated};
use candidates::{Candidate, load_candidates};

pub(crate) use records::Member;
pub use records::{
    ApplyStatus, BaselineStatus, CandidateVerdict, ReproVerdict, RunHeader, ValidateReport, Verdict,
};
pub use runner::{EnvRunner, TestRunner};

/// The bound of one command, in seconds, when the caller names none.
pub const DEFAULT_TIMEOUT_SECONDS: usize = 300;
/// The representatives that run at once when the caller names no number.
pub const DEFAULT_JOBS: usize = 1;
/// The baseline runs when the caller names no number: a repeat costs a full test run.
pub const DEFAULT_REPEAT_BASELINE: usize = 1;

const MAX_CANDIDATES: usize = 200;
// One worker per candidate is the most a run can use, so a larger number is refused.
const MAX_JOBS: usize = MAX_CANDIDATES;
const MAX_CANDIDATE_BYTES: usize = io::MAX_FILE_BYTES;
const INTERRUPTED: &str = "validate was interrupted by SIGINT; no verdicts were written";

/// The inputs of one validate run, all of them from the invocation. The numbers stay as the
/// caller wrote them until validate checks them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidateRequest {
    /// The directory of candidate patches, one file per candidate. A leading `~` is the home
    /// directory, and a relative path is below the start-up directory.
    pub candidates: PathBuf,
    /// The test command.
    pub test_cmd: String,
    /// The reproduction command, which must fail on unpatched HEAD to count.
    pub repro_cmd: Option<String>,
    /// The bound of one command, in seconds: at least 1.
    pub timeout: IntArg,
    /// The representatives that run at once: 1 through 200.
    pub jobs: IntArg,
    /// The baseline runs before any candidate: at least 1.
    pub repeat_baseline: IntArg,
    /// The bound of the whole run, in seconds: at least 1, or none.
    pub run_timeout: Option<IntArg>,
    /// The names of more variables that each command inherits when they are set.
    pub pass_env: Vec<String>,
    /// True when the test command was read from the repository under analysis.
    pub test_cmd_from_repo: bool,
    /// True when the caller accepts a test command that the repository named.
    pub allow_repo_test_cmd: bool,
    /// True when a candidate may edit the files that name what the tests run.
    pub allow_test_config_edits: bool,
}

/// Validates every candidate in `request.candidates` against `repo` at the commit that
/// `snapshot` names: the baseline, the reproduction baseline, then each distinct candidate
/// result in its own worktree. The diagnostics are the ignored cache-home warning, once, and
/// the group-kill, temporary-file and worktree-cleanup lines in run order, with either outcome.
///
/// # Errors
///
/// Returns an operation failure for a repository-named command without the opt-in, a bound
/// that bounds nothing, an unusable candidate set, a failed apply write or diff, and a failed
/// capture file, wait or watch of a test run; an input-unreadable error for the candidates; a
/// repository-resolution error for a missing commit or a refused worktree; a security refusal
/// for a candidate path outside the repository; and a stopped call after a SIGINT.
#[must_use]
pub fn validate(
    services: &Services,
    repo: &Repo,
    snapshot: &git::Snapshot,
    request: &ValidateRequest,
    runner: &dyn TestRunner,
) -> Noted<Result<ValidateReport, Error>> {
    let admitted = match admitted(services, repo, snapshot, request) {
        Ok(admitted) => admitted,
        Err(error) => return Noted::new(Err(error)),
    };
    let run = Run {
        services,
        runner,
        request,
        root: &repo.root,
        commit: admitted.commit,
        limits: admitted.limits,
        deadline: admitted.deadline,
        stop: AtomicBool::new(false),
        warned: AtomicBool::new(false),
    };
    run.report(&admitted.candidates)
}

// What every phase of one call shares: one deadline, and the one commit every worktree holds.
struct Admitted {
    limits: Limits,
    deadline: Option<Instant>,
    candidates: Vec<Candidate>,
    commit: String,
}

// The refusals that come before anything runs, in the order the caller meets them.
fn admitted(
    services: &Services,
    repo: &Repo,
    snapshot: &git::Snapshot,
    request: &ValidateRequest,
) -> Result<Admitted, Error> {
    refuse_unowned_command(request)?;
    let limits = budget::checked(request)?;
    let deadline = budget::deadline(services.clock.now(), limits.budget);
    let candidates = load_candidates(services, &request.candidates)?;
    let head = snapshot.head_sha.as_deref();
    let commit = services.git.pinned_commit(&repo.root, head);
    Ok(Admitted {
        limits,
        deadline,
        candidates,
        commit: commit.map_err(kernel::Error::from)?,
    })
}

// The guard keys on where the command came from, not on which caller asks.
fn refuse_unowned_command(request: &ValidateRequest) -> Result<(), Error> {
    if !request.test_cmd_from_repo || request.allow_repo_test_cmd {
        return Ok(());
    }
    let message = format!(
        "refusing to run {}: it came from the repository under analysis, not from the \
         invocation, so the repository would be choosing the command that judges it. Pass the \
         command explicitly, or opt in with --allow-repo-test-cmd.",
        text::CodePointString::from(request.test_cmd.as_str()).quoted()
    );
    Err(kernel::Error::OperationFailed(message).into())
}

// One validate call: its ports, its checked numbers, and the flags its workers share.
struct Run<'a> {
    services: &'a Services,
    runner: &'a dyn TestRunner,
    request: &'a ValidateRequest,
    root: &'a Path,
    commit: String,
    limits: Limits,
    deadline: Option<Instant>,
    // Set by the first failed or interrupted representative: no further one starts.
    stop: AtomicBool,
    warned: AtomicBool,
}

impl Run<'_> {
    fn report(&self, candidates: &[Candidate]) -> Noted<Result<ValidateReport, Error>> {
        and_then(self.baseline(candidates.len()), |(header, expired)| {
            if expired {
                let budget = self.budget();
                let verdicts = candidates
                    .iter()
                    .map(|candidate| deadline_expired(candidate, &budget));
                let report = ValidateReport {
                    header,
                    verdicts: verdicts.collect(),
                    deadline_expired: true,
                };
                return Noted::new(Ok(report));
            }
            if header.baseline == BaselineStatus::Unverified {
                let report = ValidateReport {
                    header,
                    verdicts: candidates.iter().map(not_evaluated).collect(),
                    deadline_expired: false,
                };
                return Noted::new(Ok(report));
            }
            let evaluated = self.evaluate_all(candidates, header.repro_valid());
            map_ok(evaluated, |(verdicts, deadline_expired)| ValidateReport {
                header,
                verdicts,
                deadline_expired,
            })
        })
    }

    fn now(&self) -> Instant {
        self.services.clock.now()
    }

    // The seconds since `started`, to 3 decimal places.
    fn elapsed(&self, started: Instant) -> f64 {
        let spent = self.now().saturating_duration_since(started);
        json::round_half_even(spent.as_secs_f64(), 3)
    }

    fn bound(&self) -> Option<Bound> {
        budget::command_bound(self.limits.timeout, self.deadline, self.now())
    }

    fn reached(&self) -> bool {
        budget::reached(self.deadline, self.now())
    }

    // The run budget as the reasons print it; "None" only when no deadline was set.
    fn budget(&self) -> String {
        self.limits
            .budget
            .map_or_else(|| "None".to_owned(), |budget| budget.to_string())
    }

    // A worktree at the run's commit for `work`, released after it. The ignored cache-home
    // warning comes once, before the first worktree.
    fn in_worktree<T>(
        &self,
        work: impl FnOnce(&Worktree) -> Noted<Result<T, Error>>,
    ) -> Noted<Result<T, Error>> {
        let cache = match &self.services.cache_root {
            Ok(cache) => cache,
            Err(home) => {
                let error = kernel::Error::OperationFailed(home.to_string());
                return Noted::new(Err(error.into()));
            }
        };
        let mut diagnostics = Vec::new();
        if !self.warned.swap(true, Ordering::SeqCst) {
            diagnostics.extend(cache.ignored.iter().map(ToString::to_string));
        }
        let tree = match self
            .services
            .git
            .worktree(self.root, &self.commit, &cache.path)
        {
            Ok(tree) => tree,
            Err(failure) => {
                diagnostics.extend(failure.cleanup.iter().map(ToString::to_string));
                let value = Err(kernel::Error::from(failure.error).into());
                return Noted { value, diagnostics };
            }
        };
        let worked = work(&tree);
        diagnostics.extend(worked.diagnostics);
        diagnostics.extend(tree.release().iter().map(ToString::to_string));
        Noted {
            value: worked.value,
            diagnostics,
        }
    }

    // One command in `cwd` under `bound`. A SIGINT during the run, or seen after it, stops the
    // call; the group-kill lines come first.
    fn run_command(
        &self,
        cwd: &Path,
        command: &str,
        bound: &Bound,
    ) -> Noted<Result<RunResult, Error>> {
        let test = TestCommand {
            cwd,
            command,
            timeout: bound.time,
            max_capture: io::DEFAULT_MAX_CAPTURE,
            pass_env: &self.request.pass_env,
        };
        let (value, notes) = match self.runner.run(&test) {
            Ok(TestRun { result, notes }) => (Ok(result), notes),
            Err(error) => unfinished(error),
        };
        let value = value.and_then(|result| {
            if self.runner.interrupted() {
                return Err(stopped());
            }
            Ok(result)
        });
        Noted {
            value,
            diagnostics: notes.iter().map(ToString::to_string).collect(),
        }
    }
}

// A run with no result: SIGINT is a stopped call, a capture, wait or watch failure an
// operation failure.
fn unfinished(error: TestRunError) -> (Result<RunResult, Error>, Vec<io::GroupNote>) {
    let message = error.to_string();
    let failed = || kernel::Error::OperationFailed(message.clone()).into();
    match error {
        TestRunError::Interrupted { notes } => (Err(stopped()), notes),
        TestRunError::Capture { notes, .. } | TestRunError::Wait { notes, .. } => {
            (Err(failed()), notes)
        }
        TestRunError::Watch(_) => (Err(failed()), Vec::new()),
    }
}

// The worktree as the repository a candidate is judged in. Its root lies below the cache root,
// which need not be a UTF-8 path, so the read operations' UTF-8 check does not apply.
fn worktree_repo(tree: &Worktree) -> Result<Repo, Error> {
    let root = io::resolve(tree.inside())
        .map_err(|error| kernel::Error::OperationFailed(io::strerror(&error)))?;
    let config = io::load_config(&root);
    Ok(Repo { root, config })
}

fn stopped() -> Error {
    kernel::Error::CallStopped(INTERRUPTED.to_owned()).into()
}

fn is_stopped(error: &Error) -> bool {
    matches!(error, Error::Kind(kernel::Error::CallStopped(_)))
}

// Maps the value of `noted` when it has one, and keeps its diagnostics.
fn map_ok<T, U>(noted: Noted<Result<T, Error>>, f: impl FnOnce(T) -> U) -> Noted<Result<U, Error>> {
    noted.map(|value| value.map(f))
}

// Runs `next` on the value of `first` when it has one; the diagnostics of both, in order.
fn and_then<T, U>(
    first: Noted<Result<T, Error>>,
    next: impl FnOnce(T) -> Noted<Result<U, Error>>,
) -> Noted<Result<U, Error>> {
    let Noted {
        value,
        mut diagnostics,
    } = first;
    match value {
        Ok(value) => {
            let second = next(value);
            diagnostics.extend(second.diagnostics);
            Noted {
                value: second.value,
                diagnostics,
            }
        }
        Err(error) => Noted {
            value: Err(error),
            diagnostics,
        },
    }
}

#[cfg(test)]
mod tests;
