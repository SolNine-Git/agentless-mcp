//! How many roots a server keeps the facts and graph of, and for how long with no call.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// The state of at most `roots` roots, each for at most `idle` after its last call. The owner sets
// both values here.
pub(super) const RETAINED: Retention = Retention {
    roots: 2,
    idle: Duration::from_secs(15 * 60),
};

pub(super) struct Retention {
    pub(super) roots: usize,
    pub(super) idle: Duration,
}

// One kept root: its path, the time of its last call, and the turn of that call among all calls.
pub(super) type Called<'p> = (&'p Path, Instant, u64);

impl Retention {
    // The roots to drop at `now`: each idle for `idle` or longer, then, when a call into `calling`
    // arrives, the least recently called beyond `roots`, `calling` counted as called now.
    pub(super) fn dropped(
        &self,
        kept: &[Called<'_>],
        calling: Option<&Path>,
        now: Instant,
    ) -> Vec<PathBuf> {
        let idle = |(_, used, _): &&Called<'_>| now.saturating_duration_since(*used) >= self.idle;
        let mut dropped: Vec<&Path> = kept.iter().filter(idle).map(|(path, ..)| *path).collect();
        if let Some(calling) = calling {
            let mut others: Vec<&Called<'_>> = (kept.iter())
                .filter(|called| !idle(called) && called.0 != calling)
                .collect();
            others.sort_by_key(|(.., turn)| *turn);
            let over = (others.len() + 1).saturating_sub(self.roots);
            dropped.extend(others.into_iter().take(over).map(|(path, ..)| *path));
        }
        dropped.into_iter().map(Path::to_path_buf).collect()
    }
}
