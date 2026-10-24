use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use io::testing::Tree;
use kernel::Clock;
use tokio::sync::mpsc;

use crate::restart::{Fingerprint, Fingerprints, Installed, Monitor, Step, step};

const A: Fingerprint = Fingerprint {
    device: 1,
    inode: 10,
    mtime_ns: 100,
};
const B: Fingerprint = Fingerprint {
    device: 1,
    inode: 11,
    mtime_ns: 100,
};
const C: Fingerprint = Fingerprint {
    device: 1,
    inode: 11,
    mtime_ns: 200,
};

fn seconds(count: u64) -> Duration {
    Duration::from_secs(count)
}

#[test]
fn each_poll_waits_adopts_holds_or_restarts() {
    let table = [
        (None, None, 120, Step::Wait(None)),
        (Some(A), None, 120, Step::Wait(Some(A))),
        (None, Some(A), 120, Step::Wait(Some(A))),
        (Some(A), Some(A), 120, Step::Wait(Some(A))),
        (Some(A), Some(B), 0, Step::Wait(Some(A))),
        (Some(A), Some(B), 59, Step::Wait(Some(A))),
        (Some(A), Some(B), 60, Step::Restart),
        (Some(B), Some(C), 3600, Step::Restart),
    ];
    for (baseline, current, uptime, want) in table {
        let got = step(baseline, current, seconds(uptime));
        assert_eq!(got, want, "{baseline:?} {current:?} {uptime}");
    }
}

// The fingerprints of the start read and of each poll, in order.
struct Scripted(Mutex<VecDeque<Option<Fingerprint>>>);

impl Fingerprints for Scripted {
    fn read(&self) -> Option<Fingerprint> {
        let mut reads = self.0.lock().expect("an unpoisoned script");
        reads.pop_front().expect("a scripted read")
    }
}

// The monotonic time of each poll, in order.
struct Uptimes {
    start: Instant,
    polls: Mutex<VecDeque<u64>>,
}

impl Clock for Uptimes {
    fn wall(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }

    fn now(&self) -> Instant {
        let mut polls = self.polls.lock().expect("an unpoisoned clock");
        self.start + seconds(polls.pop_front().expect("a scripted poll"))
    }
}

// Runs the watch over one tick for each uptime, then ends the ticks: true when it asked for a
// restart, with the reads it left unread.
fn watched(reads: &[Option<Fingerprint>], uptimes: &[u64]) -> (bool, usize) {
    let source = Arc::new(Scripted(Mutex::new(reads.iter().copied().collect())));
    let start = Instant::now();
    let clock = Uptimes {
        start,
        polls: Mutex::new(uptimes.iter().copied().collect()),
    };
    let monitor = Monitor {
        source: Arc::clone(&source) as Arc<dyn Fingerprints>,
        clock: Arc::new(clock),
        started: start,
    };
    let (sender, ticks) = mpsc::channel(uptimes.len().max(1));
    for _ in uptimes {
        sender.try_send(()).expect("room for each tick");
    }
    drop(sender);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime");
    let restart = runtime.block_on(monitor.watch(ticks));
    let left = source.0.lock().expect("an unpoisoned script").len();
    (restart, left)
}

#[test]
fn a_change_under_the_minimum_uptime_does_not_restart() {
    assert_eq!(watched(&[Some(A), Some(B), Some(B)], &[10, 59]), (false, 0));
}

#[test]
fn a_held_change_restarts_once_the_uptime_passes() {
    assert_eq!(
        watched(&[Some(A), Some(B), Some(B), Some(B)], &[10, 60, 90]),
        (true, 1)
    );
}

#[test]
fn an_absent_fingerprint_never_restarts() {
    assert_eq!(watched(&[Some(A), None, None], &[100, 200]), (false, 0));
}

#[test]
fn an_absent_start_adopts_the_first_fingerprint_read() {
    assert_eq!(watched(&[None, Some(B), Some(B)], &[100, 200]), (false, 0));
    assert_eq!(watched(&[None, Some(B), Some(C)], &[100, 200]), (true, 0));
}

#[test]
fn a_change_after_the_minimum_uptime_restarts_at_that_poll() {
    assert_eq!(watched(&[Some(A), Some(B), Some(B)], &[61, 62]), (true, 1));
}

// A replaced file has a new inode; a missing one has no fingerprint.
#[test]
fn the_installed_fingerprint_follows_the_file_at_its_path() {
    let tree = Tree::new().expect("a scratch directory");
    tree.write_file("bin", b"one").expect("a binary");
    let installed = Installed(tree.join("bin"));
    let first = installed.read().expect("a fingerprint");
    assert_eq!(installed.read(), Some(first));
    io::replace_file(&tree.join("bin"), b"two").expect("a replaced binary");
    let second = installed.read().expect("a fingerprint");
    assert_ne!(first.inode, second.inode);
    assert_eq!(step(Some(first), Some(second), seconds(60)), Step::Restart);
    assert!(io::remove_file(&tree.join("bin")).expect("a removal"));
    assert_eq!(installed.read(), None);
}
