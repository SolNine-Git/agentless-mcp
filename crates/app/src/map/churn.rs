//! Each ranked file's commit activity inside the churn window: data beside the rank, never in it.

use std::time::{SystemTime, UNIX_EPOCH};

use super::MapFile;
use crate::{Reading, Services};

const SECONDS_PER_DAY: i64 = 86_400;

// One git call for the whole listing. A root without a HEAD, or a log git cannot answer, leaves
// the files bare, so a map never claims a quiet history it did not measure.
pub(super) fn with_churn(
    services: &Services,
    reading: &Reading<'_>,
    files: Vec<MapFile>,
) -> Vec<MapFile> {
    if reading.snapshot.head_sha.is_none() || files.is_empty() {
        return files;
    }
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let Some(facts) = services.git.churn(&reading.repo.root, &paths) else {
        return files;
    };
    let now = unix_seconds(services.clock.wall());
    let stamp = |mut file: MapFile| {
        if let Some(fact) = facts.get(&file.path) {
            file.commits_window = Some(fact.commits);
            file.last_commit_days = fact.last_commit_ts.map(|stamp| days_since(now, stamp));
        }
        file
    };
    files.into_iter().map(stamp).collect()
}

// A commit dated in the future is 0 days old.
fn days_since(now: i64, stamp: i64) -> i64 {
    now.saturating_sub(stamp).div_euclid(SECONDS_PER_DAY).max(0)
}

// Whole seconds since the epoch, cut toward zero on either side of it.
fn unix_seconds(wall: SystemTime) -> i64 {
    let whole = |seconds: u64| i64::try_from(seconds).unwrap_or(i64::MAX);
    match wall.duration_since(UNIX_EPOCH) {
        Ok(after) => whole(after.as_secs()),
        Err(before) => -whole(before.duration().as_secs()),
    }
}

#[cfg(test)]
mod tests;
