//! The restart on upgrade: the fingerprint of the installed binary, polled, and the exec of that
//! path with this process's arguments once the fingerprint changes.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kernel::Clock;
use tokio::sync::mpsc;

// An upgrade is a human-timescale event: a late restart costs a few answers from the old code.
pub(crate) const POLL: Duration = Duration::from_secs(30);
// A change seen sooner is held, never dropped, so the rate of restarts is bounded.
pub(crate) const MINIMUM_UPTIME: Duration = Duration::from_secs(60);

/// The installed binary to run in place of this process, with the arguments this process got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relaunch {
    /// The binary's path, resolved once at start-up.
    pub program: PathBuf,
    /// The program name this process got.
    pub argv0: OsString,
    /// The arguments after the program name.
    pub args: Vec<OsString>,
}

impl Relaunch {
    /// Replaces this process with `program`; when the exec fails, returns its log line.
    #[must_use]
    pub fn exec(&self) -> String {
        let error = Command::new(&self.program)
            .arg0(&self.argv0)
            .args(&self.args)
            .exec();
        format!(
            "install updated but exec of {} failed; this process is exiting 0 and will NOT come \
             back on its own -- the service returns only when a supervisor or an operator starts \
             it again\n{}",
            self.program.display(),
            io::os_error_line(&error)
        )
    }
}

// The file's identity: a replaced binary has a new inode, a rewritten one a new mtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    pub(crate) device: u64,
    pub(crate) inode: u64,
    pub(crate) mtime_ns: i128,
}

pub(crate) trait Fingerprints: Send + Sync {
    // The fingerprint now, or `None` while the file cannot be read, as during an upgrade.
    fn read(&self) -> Option<Fingerprint>;
}

// The path stored at start-up. The running image's own link names a deleted file after an
// upgrade, so it is never read here.
pub(crate) struct Installed(pub(crate) PathBuf);

impl Fingerprints for Installed {
    fn read(&self) -> Option<Fingerprint> {
        let metadata = fs::metadata(&self.0).ok()?;
        let nanos =
            i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
        Some(Fingerprint {
            device: metadata.dev(),
            inode: metadata.ino(),
            mtime_ns: nanos,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    // Keep polling, against this baseline.
    Wait(Option<Fingerprint>),
    Restart,
}

// An absent fingerprint is an upgrade in progress, so absence never restarts; the first one read
// becomes the baseline. A change under the minimum uptime keeps the old baseline, so it is held.
pub(crate) fn step(
    baseline: Option<Fingerprint>,
    current: Option<Fingerprint>,
    uptime: Duration,
) -> Step {
    match (baseline, current) {
        (_, None) => Step::Wait(baseline),
        (None, Some(current)) => Step::Wait(Some(current)),
        (Some(old), Some(new)) if old == new || uptime < MINIMUM_UPTIME => Step::Wait(Some(old)),
        _ => Step::Restart,
    }
}

// The watch of one HTTP server: its fingerprint source and the clock of its uptime.
pub(crate) struct Monitor {
    pub(crate) source: Arc<dyn Fingerprints>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) started: Instant,
}

impl Monitor {
    // One read for each tick, against the fingerprint at the start; true when a restart is due,
    // false when the ticks end first.
    pub(crate) async fn watch(&self, mut ticks: mpsc::Receiver<()>) -> bool {
        let mut baseline = self.source.read();
        while ticks.recv().await.is_some() {
            let uptime = self.clock.now().saturating_duration_since(self.started);
            match step(baseline, self.source.read(), uptime) {
                Step::Wait(next) => baseline = next,
                Step::Restart => return true,
            }
        }
        false
    }
}

// One tick each poll interval, until the receiver is gone.
pub(crate) async fn ticking(sender: mpsc::Sender<()>) {
    loop {
        tokio::time::sleep(POLL).await;
        if sender.send(()).await.is_err() {
            return;
        }
    }
}
