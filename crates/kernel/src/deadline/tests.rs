use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use super::{Deadline, Stopped, checkpoint, recaching, walk_budget};
use crate::testing::PinnedClock;

const CAP: Duration = Duration::from_secs(10);

fn pinned() -> Arc<PinnedClock> {
    Arc::new(PinnedClock::at(UNIX_EPOCH))
}

fn seconds(value: f64) -> Duration {
    Duration::from_secs_f64(value)
}

#[test]
fn a_checkpoint_with_no_deadline_never_stops() {
    assert_eq!(checkpoint(None), Ok(()));
}

#[test]
fn a_spent_allowance_stops_the_call() {
    let clock = pinned();
    let call = Deadline::new(5.0, 7.0, clock.clone());
    clock.advance(seconds(5.0 + 7.0 + 60.0));

    let stopped = checkpoint(Some(&call));

    let spent = Stopped::Spent {
        elapsed: 72.0,
        limit: 5.0,
        grace: 7.0,
        credited: 0.0,
    };
    assert_eq!(stopped, Err(spent));
}

#[test]
fn a_cancelled_call_stops_at_its_next_checkpoint() {
    let call = Deadline::new(60.0, 0.0, pinned());
    assert_eq!(checkpoint(Some(&call)), Ok(()));

    call.cancel();

    assert_eq!(checkpoint(Some(&call)), Err(Stopped::Cancelled));
}

#[test]
fn parse_time_extends_the_allowance_up_to_the_grace() {
    let clock = pinned();
    let call = Deadline::new(10.0, 30.0, clock.clone());

    recaching(Some(&call), || clock.advance(seconds(20.0)));
    clock.advance(seconds(10.0));
    assert_eq!(checkpoint(Some(&call)), Ok(()));
    recaching(Some(&call), || clock.advance(seconds(20.0)));
    let stopped = checkpoint(Some(&call));

    assert!(matches!(stopped, Err(Stopped::Spent { .. })), "{stopped:?}");
    assert_eq!(call.credited().to_bits(), 30.0_f64.to_bits());
}

#[test]
fn the_remaining_allowance_counts_the_credited_grace() {
    let clock = pinned();
    let call = Deadline::new(30.0, 60.0, clock.clone());

    recaching(Some(&call), || clock.advance(seconds(5.0)));
    clock.advance(seconds(5.0));

    assert_eq!(
        call.remaining().to_bits(),
        (30.0_f64 + 5.0 - 10.0).to_bits()
    );
}

#[test]
fn a_spent_allowance_leaves_a_negative_remainder() {
    let clock = pinned();
    let call = Deadline::new(10.0, 0.0, clock.clone());
    clock.advance(seconds(12.0));

    assert_eq!(call.remaining().to_bits(), (-2.0_f64).to_bits());
}

#[test]
fn recaching_with_no_deadline_runs_the_work_and_credits_nothing() {
    let clock = pinned();
    let value = recaching(None, || {
        clock.advance(seconds(5.0));
        7
    });
    assert_eq!(value, 7);
}

#[test]
fn the_walk_budget_is_the_lesser_of_the_cap_and_half_the_remainder() {
    let cases = [
        (None, 10.0),
        (Some(30.0), 10.0),
        (Some(8.0), 4.0),
        (Some(-1.0), 0.0),
    ];
    for (remaining, budget) in cases {
        assert_eq!(
            walk_budget(CAP, remaining),
            seconds(budget),
            "{remaining:?}"
        );
    }
}

#[test]
fn a_nan_grace_and_a_nan_remainder_keep_the_reference_order() {
    let call = Deadline::new(1.0, f64::NAN, pinned());
    call.credit(5.0);
    assert!(call.credited().is_nan());
    assert_eq!(walk_budget(CAP, Some(f64::NAN)), CAP);
}
