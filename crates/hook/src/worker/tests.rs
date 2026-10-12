use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use app::{After, LintForm, LintReport, LintRequest, PairSelection, Repo, Services};

use super::{Job, LintFn, Linted, run};
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

const SLOW: LintFn = |_, _, _| {
    thread::sleep(Duration::from_millis(300));
    Ok(LintReport {
        candidates: Vec::new(),
    })
};

const FAILING: LintFn =
    |_, _, _| Err(kernel::Error::OperationFailed("no cache root".to_owned()).into());

const PANICKING: LintFn = |_, _, _| panic!("a defect in lint");

#[test]
fn a_report_in_time_comes_back() {
    let sandbox = Sandbox::new();
    let linted = run(REPORTED, job(&sandbox), Duration::from_secs(5)).expect("the thread starts");
    assert!(matches!(linted, Linted::Report(_)), "{linted:?}");
}

#[test]
fn a_lint_that_outlives_the_budget_times_out_without_a_wait_for_it() {
    let sandbox = Sandbox::new();
    let started = Instant::now();
    let linted = run(SLOW, job(&sandbox), Duration::from_millis(50)).expect("the thread starts");
    assert!(matches!(linted, Linted::TimedOut), "{linted:?}");
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_failure_and_a_panic_come_back_as_values() {
    let sandbox = Sandbox::new();
    let failed = run(FAILING, job(&sandbox), Duration::from_secs(5)).expect("the thread starts");
    assert!(matches!(failed, Linted::Failed(_)), "{failed:?}");
    let panicked =
        run(PANICKING, job(&sandbox), Duration::from_secs(5)).expect("the thread starts");
    let Linted::Panicked(message) = panicked else {
        panic!("{panicked:?}");
    };
    assert_eq!(message, "a defect in lint");
}
