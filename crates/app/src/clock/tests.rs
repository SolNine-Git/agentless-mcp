use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{Clock, PinnedClock, SystemClock};
use crate::{EnvConfig, Services};

const WALL_SECONDS: u64 = 1_800_000_000;

#[test]
fn the_system_clock_reads_the_operating_system_clocks() {
    let before = SystemTime::now();
    let started = Instant::now();
    let (wall, now) = (SystemClock.wall(), SystemClock.now());
    assert!(wall >= before);
    assert!(now >= started);
}

#[test]
fn a_pinned_clock_holds_its_wall_time_and_moves_its_monotonic_time_only_when_told() {
    let wall = UNIX_EPOCH + Duration::from_secs(WALL_SECONDS);
    let clock = PinnedClock::at(wall);
    let start = clock.now();
    assert_eq!((clock.wall(), clock.now()), (wall, start));
    clock.advance(Duration::from_secs(11));
    assert_eq!(clock.now() - start, Duration::from_secs(11));
    assert_eq!(clock.wall(), wall);
}

// The services that every operation reads carry one clock, and a clone reads the same one.
#[test]
fn the_services_clock_is_the_one_a_test_pins() {
    let mut services = Services::new(&EnvConfig::new([], PathBuf::from("/")));
    let wall = UNIX_EPOCH + Duration::from_secs(WALL_SECONDS);
    let pinned = Arc::new(PinnedClock::at(wall));
    services.clock = pinned.clone();
    let copy = services.clone();
    pinned.advance(Duration::from_secs(3));
    assert_eq!(copy.clock.wall(), wall);
    assert_eq!(copy.clock.now(), pinned.now());
}

#[test]
fn new_services_count_tokens_with_chars4() {
    let services = Services::new(&EnvConfig::new([], PathBuf::from("/")));
    assert_eq!(services.counter.count("abcdefgh"), 2);
    assert!(services.clock.wall() > UNIX_EPOCH);
}
