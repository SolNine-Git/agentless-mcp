//! A symbol card's last change: one `git log -L` commit, read on its own thread so that the card
//! waits for it no longer than its deadline. A late git keeps running until its own time limit.
//! A file card's last change: one `git log -1` commit, which git stops at the deadline itself.

use std::panic;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use git::LastCommit;
use store::FactSource;

use super::Change;
use crate::ops::limits::MIN_BUDGET;
use crate::{HistoryRequest, IntArg, Reading, Repo, Services, history};

// What the history thread owns: a reading borrows, and the thread can outlive the call.
struct Job {
    services: Services,
    repo: Repo,
    snapshot: git::Snapshot,
    target: String,
}

impl Job {
    // The thread parses the file itself, as the cache that the call reads is not its to share.
    fn run(&self) -> Change {
        let source = FactSource::parsing();
        let reading = Reading {
            repo: &self.repo,
            snapshot: &self.snapshot,
            facts: &source,
            deadline: None,
        };
        let request = HistoryRequest {
            target: self.target.clone(),
            limit: IntArg::from_usize(1),
            budget: IntArg::from_usize(MIN_BUDGET),
        };
        match history(&self.services, &reading, &request).value {
            Ok(found) => {
                let first = found.entries.into_iter().next();
                Change::Commit(first.expect("history refuses a span with no commit"))
            }
            Err(error) => Change::Unavailable(error.to_string()),
        }
    }
}

pub(super) fn last_change(
    services: &Services,
    reading: &Reading<'_>,
    target: &str,
    wait: Duration,
) -> Change {
    let job = Job {
        services: services.clone(),
        repo: reading.repo.clone(),
        snapshot: reading.snapshot.clone(),
        target: target.to_owned(),
    };
    let (sender, receiver) = mpsc::channel();
    let spawned = thread::Builder::new().spawn(move || {
        // The send fails only when the card stopped waiting, and then nothing reads the change.
        let _ = sender.send(job.run());
    });
    match spawned {
        Ok(handle) => awaited(&receiver, handle, wait),
        Err(error) => Change::Unavailable(error.to_string()),
    }
}

// The change that arrives within `wait`. A thread that ends with no change panicked, and the
// panic reaches the caller.
fn awaited(receiver: &Receiver<Change>, handle: JoinHandle<()>, wait: Duration) -> Change {
    match receiver.recv_timeout(wait) {
        Ok(change) => change,
        Err(RecvTimeoutError::Timeout) => Change::NotRead {
            seconds: wait.as_secs(),
        },
        Err(RecvTimeoutError::Disconnected) => match handle.join() {
            Err(payload) => panic::resume_unwind(payload),
            Ok(()) => unreachable!("the history thread sends before it ends"),
        },
    }
}

pub(super) fn file_change(
    services: &Services,
    reading: &Reading<'_>,
    path: &str,
    wait: Duration,
) -> Change {
    match services.git.last_commit(&reading.repo.root, path, wait) {
        LastCommit::Found(commit) => Change::Commit(crate::history::entry(commit)),
        LastCommit::NoCommit => Change::NoCommit,
        LastCommit::NotRepository => Change::NotRepository {
            note: reading.snapshot.note.clone(),
        },
        LastCommit::TimedOut => Change::NotRead {
            seconds: wait.as_secs(),
        },
        LastCommit::Failed(note) => Change::Failed(note),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::{Change, awaited};

    #[test]
    fn a_change_that_has_not_arrived_when_the_wait_ends_is_not_read() {
        let (sender, receiver) = mpsc::channel::<Change>();
        let handle = thread::spawn(|| {});
        let change = awaited(&receiver, handle, Duration::ZERO);
        assert_eq!(change, Change::NotRead { seconds: 0 });
        drop(sender);
    }

    #[test]
    fn a_change_that_arrived_is_the_card_change() {
        let (sender, receiver) = mpsc::channel();
        let sent = Change::Unavailable("no git".to_owned());
        sender.send(sent.clone()).expect("an open channel");
        let handle = thread::spawn(|| {});
        assert_eq!(awaited(&receiver, handle, Duration::ZERO), sent);
    }
}
