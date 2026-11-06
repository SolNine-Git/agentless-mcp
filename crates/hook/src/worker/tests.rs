use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use app::{After, LintForm, LintReport, LintRequest, PairSelection, Repo, Services};

use super::{Finished, Job, LintFn, run};
use crate::testing::Sandbox;

fn job(sandbox: &Sandbox) -> Job {
    Job {
        services: Services::new(&sandbox.env(&[])),
        repo: Repo {
            root: PathBuf::from(sandbox.path()),
            config: io::ProjectConfig::default(),
        },
        request: LintRequest {
            form: LintForm::Proposed {
                path: "a.py".to_owned(),
                after: After::Text(String::new()),
            },
            checks: PairSelection::edited_files(),
        },
    }
}

const REPORTED: LintFn = |_, _, _| {
    Ok(LintReport {
        candidates: Vec::new(),
    })
};

// A lint that never returns: its thread parks until the test process exits.
const STUCK: LintFn = |_, _, _| {
    loop {
        thread::park();
    }
};

const FAILING: LintFn =
    |_, _, _| Err(kernel::Error::OperationFailed("no cache root".to_owned()).into());

const PANICKING: LintFn = |_, _, _| panic!("a defect in lint");

fn ran(lint: LintFn, sandbox: &Sandbox, budget: Duration) -> Finished<LintReport> {
    run("test-lint", job(sandbox).linted_by(lint), budget).expect("the thread starts")
}

#[test]
fn a_report_in_time_comes_back() {
    let sandbox = Sandbox::new();
    let linted = ran(REPORTED, &sandbox, Duration::from_secs(5));
    assert!(matches!(linted, Finished::Done(_)), "{linted:?}");
}

#[test]
fn a_lint_that_outlives_the_budget_times_out_without_a_wait_for_it() {
    let sandbox = Sandbox::new();
    let started = Instant::now();
    let linted = ran(STUCK, &sandbox, Duration::from_millis(50));
    assert!(matches!(linted, Finished::TimedOut), "{linted:?}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_failure_and_a_panic_come_back_as_values() {
    let sandbox = Sandbox::new();
    let failed = ran(FAILING, &sandbox, Duration::from_secs(5));
    assert!(matches!(failed, Finished::Failed(_)), "{failed:?}");
    let panicked = ran(PANICKING, &sandbox, Duration::from_secs(5));
    let Finished::Panicked(message) = panicked else {
        panic!("{panicked:?}");
    };
    assert_eq!(message, "a defect in lint");
}
