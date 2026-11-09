//! How many states a server keeps the facts and graph of, and for how long with no call.

use std::time::{Duration, Instant};

// At most `roots` states, each for at most `idle` after its last call: a root's working tree and
// the tree of each commit that a call read count one state each. The owner sets both values here.
pub(super) const RETAINED: Retention = Retention {
    roots: 2,
    idle: Duration::from_secs(15 * 60),
};

pub(super) struct Retention {
    pub(super) roots: usize,
    pub(super) idle: Duration,
}

// An update resolves at most `sites` percent of a scan's sites again; a kept state holds at most
// `scans` scans, and replaced facts of at most `replaced` percent of its sites.
pub(super) const UPDATED: Updates = Updates {
    sites: 70,
    scans: 64,
    replaced: 8,
};

#[derive(Clone, Copy, Debug)]
pub(super) struct Updates {
    pub(super) sites: usize,
    pub(super) scans: usize,
    pub(super) replaced: usize,
}

impl Default for Updates {
    fn default() -> Updates {
        UPDATED
    }
}

// One kept state: its key, the time of its last call, and the turn of that call among all calls.
pub(super) type Called<'k, K> = (&'k K, Instant, u64);

impl Retention {
    // The states to drop at `now`: each idle for `idle` or longer, then, when a call into
    // `calling` arrives, the least recently called beyond `roots`, `calling` counted as called now.
    pub(super) fn dropped<K: PartialEq + Clone>(
        &self,
        kept: &[Called<'_, K>],
        calling: Option<&K>,
        now: Instant,
    ) -> Vec<K> {
        let idle =
            |(_, used, _): &&Called<'_, K>| now.saturating_duration_since(*used) >= self.idle;
        let mut dropped: Vec<&K> = kept.iter().filter(idle).map(|(key, ..)| *key).collect();
        if let Some(calling) = calling {
            let mut others: Vec<&Called<'_, K>> = (kept.iter())
                .filter(|called| !idle(called) && called.0 != calling)
                .collect();
            others.sort_by_key(|(.., turn)| *turn);
            let over = (others.len() + 1).saturating_sub(self.roots);
            dropped.extend(others.into_iter().take(over).map(|(key, ..)| *key));
        }
        dropped.into_iter().cloned().collect()
    }
}
