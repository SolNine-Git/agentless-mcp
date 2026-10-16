use std::time::{Duration, Instant};

use io::{RunResult, RunStatus};

use super::{Bound, MIN_COMMAND, command_bound, deadline, reached, truncated};

fn run(status: RunStatus) -> RunResult {
    RunResult {
        status,
        exit_code: None,
        duration: 1.0,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
    }
}

#[test]
fn no_deadline_gives_the_command_its_own_timeout() {
    let now = Instant::now();

    let bound = command_bound(300, None, now);

    let own = Bound {
        time: Duration::from_secs(300),
        from_budget: false,
    };
    assert_eq!(bound, Some(own));
}

#[test]
fn the_budget_left_bounds_a_command_when_it_is_shorter() {
    let now = Instant::now();
    let at = deadline(now, Some(100));
    let later = now + Duration::from_millis(50_500);

    let bound = command_bound(60, at, later);

    let left = Bound {
        time: Duration::from_millis(49_500),
        from_budget: true,
    };
    assert_eq!(bound, Some(left));
    assert_eq!(
        command_bound(60, at, now).map(|bound| bound.from_budget),
        Some(false)
    );
}

#[test]
fn less_than_one_second_left_gives_no_bound() {
    let now = Instant::now();
    let at = deadline(now, Some(10));

    assert_eq!(
        command_bound(300, at, now + Duration::from_secs(9)),
        Some(Bound {
            time: MIN_COMMAND,
            from_budget: true,
        })
    );
    assert_eq!(
        command_bound(300, at, now + Duration::from_millis(9_001)),
        None
    );
    assert_eq!(command_bound(300, at, now + Duration::from_secs(20)), None);
}

#[test]
fn the_deadline_is_reached_at_its_instant() {
    let now = Instant::now();
    let at = deadline(now, Some(5));

    assert!(!reached(at, now + Duration::from_millis(4_999)));
    assert!(reached(at, now + Duration::from_secs(5)));
    assert!(!reached(None, now + Duration::from_secs(1 << 40)));
}

#[test]
fn a_budget_past_the_clock_range_is_no_deadline() {
    let now = Instant::now();

    assert_eq!(deadline(now, Some(usize::MAX)), None);
    assert_eq!(deadline(now, None), None);
}

#[test]
fn only_a_timeout_under_a_budget_bound_is_truncated() {
    let budget = Bound {
        time: MIN_COMMAND,
        from_budget: true,
    };
    let own = Bound {
        from_budget: false,
        ..budget
    };

    assert!(truncated(&budget, &run(RunStatus::Timeout)));
    assert!(!truncated(&budget, &run(RunStatus::Failed)));
    assert!(!truncated(&own, &run(RunStatus::Timeout)));
}
