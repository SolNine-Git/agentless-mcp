use io::RunStatus;

use super::{NOT_STARTED, expired_baseline};
use crate::validate::BaselineStatus;

#[test]
fn a_budget_spent_before_the_first_repeat_reports_a_timeout_that_never_started() {
    let outcome = expired_baseline(&[], 2, "5");

    assert_eq!(outcome.status, BaselineStatus::Unverified);
    assert_eq!(
        outcome.detail,
        "the run's 5s budget expired after 0 of 2 required baseline runs"
    );
    let started = outcome.representative.expect("a representative run");
    assert_eq!(started.status, RunStatus::Timeout);
    assert_eq!(started.exit_code, None);
    assert_eq!(started.stderr_tail, NOT_STARTED);
    assert_eq!((outcome.failures, outcome.deadline_expired), (2, true));
}
