use std::time::{Duration, UNIX_EPOCH};

use super::{days_since, unix_seconds};

#[test]
fn days_floor_and_a_future_commit_is_zero_days_old() {
    assert_eq!(days_since(86_400 * 3 + 5, 0), 3);
    assert_eq!(days_since(86_399, 0), 0);
    assert_eq!(days_since(0, 86_400 * 2), 0);
    assert_eq!(days_since(i64::MIN, i64::MAX), 0);
}

#[test]
fn seconds_cut_toward_zero_on_either_side_of_the_epoch() {
    assert_eq!(unix_seconds(UNIX_EPOCH + Duration::from_millis(1_500)), 1);
    assert_eq!(unix_seconds(UNIX_EPOCH - Duration::from_millis(1_500)), -1);
}
