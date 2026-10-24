//! The byte ceiling on the cache root: its setting, the least-recently-used eviction plan, and
//! the sweep that deletes the planned agentless.db files under their locks.

use std::cmp::Ordering;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use io::LockError;
use text::{CodePointString, IntText, convertible_int, is_space};

use crate::discard::{FACTS_LOCK_NAME, SUFFIXES, remove_database, with_suffix};
use crate::warnings::Failed;
use crate::{EvictionReport, FACTS_DATABASE_NAME};

/// The ceiling when the environment sets none: 5 GiB.
pub const DEFAULT_MAX_CACHE_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// A database used within this many seconds is never evicted.
pub const PROTECTED_SECONDS: f64 = 86_400.0;
/// The environment variable that sets the ceiling.
pub const MAX_CACHE_BYTES_VARIABLE: &str = "AGENTLESS_MCP_MAX_CACHE_BYTES";

/// One cached database, as the sweep weighs it.
#[derive(Clone, Debug, PartialEq)]
pub struct CacheEntry {
    /// The database file.
    pub database: PathBuf,
    /// The bytes of the file and its sidecars.
    pub size: u64,
    /// The modification time in seconds since the epoch, which a read refreshes.
    pub used_at: f64,
}

/// Reads the ceiling from the environment value `value`: `None` disables the sweep. A value that
/// is not a non-negative integer keeps the default ceiling and gives the warning.
#[must_use]
pub fn max_cache_bytes(value: Option<&OsStr>) -> (Option<u64>, Option<String>) {
    let stripped = stripped(value.map_or(&[][..], OsStr::as_bytes));
    if stripped.code_points().next().is_none() {
        return (Some(DEFAULT_MAX_CACHE_BYTES), None);
    }
    match stripped
        .as_str()
        .and_then(convertible_int)
        .and_then(ceiling)
    {
        Some(0) => (None, None),
        Some(limit) => (Some(limit), None),
        None => (Some(DEFAULT_MAX_CACHE_BYTES), Some(ignored(&stripped))),
    }
}

// The value without whitespace at either end. An undecodable byte is a code point that is not
// whitespace.
fn stripped(bytes: &[u8]) -> CodePointString {
    let points: Vec<u32> = CodePointString::from_utf8_escaped(bytes)
        .code_points()
        .collect();
    let space = |point: &u32| char::from_u32(*point).is_some_and(is_space);
    let start = points
        .iter()
        .position(|point| !space(point))
        .unwrap_or(points.len());
    let end = points
        .iter()
        .rposition(|point| !space(point))
        .map_or(start, |last| last + 1);
    let mut kept = CodePointString::default();
    points[start..end]
        .iter()
        .for_each(|point| kept.push(*point));
    kept
}

fn ceiling(found: IntText<'_>) -> Option<u64> {
    let value = found.digits().fold(0_u64, |value, digit| {
        value.saturating_mul(10).saturating_add(u64::from(digit))
    });
    (!found.negative || value == 0).then_some(value)
}

fn ignored(stripped: &CodePointString) -> String {
    format!(
        "{MAX_CACHE_BYTES_VARIABLE}={} is not a non-negative integer and was ignored; the \
         default ceiling of {DEFAULT_MAX_CACHE_BYTES} bytes applies. Set it to 0 to disable the \
         sweep",
        stripped.quoted()
    )
}

/// Returns the databases to delete, coldest first. The most recently used entry, `keep`, and
/// every entry used at or after `now - PROTECTED_SECONDS` are never planned, and they count
/// toward the total. Walking from the most recent, an entry is kept while the total stays at or
/// under `limit`; the first entry that does not fit is planned, and so is every colder one.
#[must_use]
pub fn evict_plan(entries: &[CacheEntry], limit: u64, keep: &Path, now: f64) -> Vec<CacheEntry> {
    let mut ordered: Vec<&CacheEntry> = entries.iter().collect();
    ordered.sort_by(|a, b| b.used_at.partial_cmp(&a.used_at).unwrap_or(Ordering::Equal));
    let protected_after = now - PROTECTED_SECONDS;
    let mut evict: Vec<CacheEntry> = Vec::new();
    let mut total: u64 = 0;
    for (position, entry) in ordered.into_iter().enumerate() {
        let protected = entry.database == keep || position == 0 || entry.used_at >= protected_after;
        let over = total.saturating_add(entry.size) > limit;
        if !protected && (!evict.is_empty() || over) {
            evict.push(entry.clone());
        } else {
            total = total.saturating_add(entry.size);
        }
    }
    evict.reverse();
    evict
}

/// Deletes the planned agentless.db files under `cache_root`, each under its own lock, when
/// `limit` is set. A victim that is locked, used again, or gone is kept and not counted. Returns
/// what was deleted and the warnings, in order.
#[must_use]
pub fn enforce_cache_limit(
    cache_root: &Path,
    keep: &Path,
    limit: Option<u64>,
    now: f64,
) -> (EvictionReport, Vec<String>) {
    let Some(limit) = limit else {
        return (EvictionReport::default(), Vec::new());
    };
    let (entries, mut warnings) = cache_entries(cache_root);
    let mut report = EvictionReport::default();
    for victim in evict_plan(&entries, limit, keep, now) {
        let (freed, said) = evicted(&victim, now);
        warnings.extend(said);
        if let Some(freed) = freed {
            report.databases += 1;
            report.size += freed;
        }
    }
    (report, warnings)
}

// Lists each directory's database, by directory name. A directory with no database is an
// evicted repository whose lock file stays.
pub(crate) fn cache_entries(root: &Path) -> (Vec<CacheEntry>, Vec<String>) {
    let mut names = match listed(root) {
        Ok(names) => names,
        Err(error) => return (Vec::new(), vec![Failed::Listing(root).warning(&error)]),
    };
    names.sort();
    let mut warnings = Vec::new();
    let mut entries = Vec::new();
    for name in names {
        let (found, said) = entry(&root.join(name).join(FACTS_DATABASE_NAME));
        warnings.extend(said);
        entries.extend(found);
    }
    (entries, warnings)
}

fn listed(root: &Path) -> std::io::Result<Vec<std::ffi::OsString>> {
    std::fs::read_dir(root)?
        .map(|found| found.map(|found| found.file_name()))
        .collect()
}

fn entry(database: &Path) -> (Option<CacheEntry>, Vec<String>) {
    let used_at = match std::fs::metadata(database) {
        Ok(found) => modified(&found),
        Err(error) if missing(&error) => return (None, Vec::new()),
        Err(error) => return (None, vec![Failed::Weighing(database, "").warning(&error)]),
    };
    let (size, warnings) = entry_bytes(database);
    let found = CacheEntry {
        database: database.to_path_buf(),
        size,
        used_at,
    };
    (Some(found), warnings)
}

// What deleting `database` and its sidecars would reclaim, the primary first. A missing sidecar
// weighs nothing.
fn entry_bytes(database: &Path) -> (u64, Vec<String>) {
    let mut total = 0;
    let mut warnings = Vec::new();
    for suffix in SUFFIXES.into_iter().rev() {
        match std::fs::metadata(with_suffix(database, suffix)) {
            Ok(found) => total += found.len(),
            Err(error) if missing(&error) => {}
            Err(error) => warnings.push(Failed::Weighing(database, suffix).warning(&error)),
        }
    }
    (total, warnings)
}

// Deletes one planned victim under its lock, and returns the bytes that left the disk.
pub(crate) fn evicted(victim: &CacheEntry, now: f64) -> (Option<u64>, Vec<String>) {
    let failed = Failed::Eviction(&victim.database);
    let lock = match io::try_lock(&victim.database.with_file_name(FACTS_LOCK_NAME)) {
        Ok(lock) => lock,
        Err(LockError::Held | LockError::Refused(_)) => return (None, Vec::new()),
        Err(LockError::Open(error)) => return (None, vec![failed.warning(&error)]),
    };
    let outcome = evict_victim(victim, now);
    drop(lock);
    outcome.unwrap_or_else(|error| (None, vec![failed.warning(&error)]))
}

// The mtime is read again under the lock: a read may have used the database since the plan.
fn evict_victim(victim: &CacheEntry, now: f64) -> std::io::Result<(Option<u64>, Vec<String>)> {
    let used_at = match std::fs::metadata(&victim.database) {
        Ok(found) => modified(&found),
        Err(error) if missing(&error) => return Ok((None, Vec::new())),
        Err(error) => {
            let warning = Failed::Recheck(&victim.database).warning(&error);
            return Ok((None, vec![warning]));
        }
    };
    if used_at >= now - PROTECTED_SECONDS {
        return Ok((None, Vec::new()));
    }
    let (freed, warnings) = entry_bytes(&victim.database);
    let removed = remove_database(&victim.database)?;
    Ok((removed.then_some(freed), warnings))
}

fn missing(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENOENT)
}

// The modification time in seconds: the whole seconds plus nanoseconds * 1e-9, as one f64 sum.
fn modified(found: &std::fs::Metadata) -> f64 {
    exact(found.mtime()) + exact(found.mtime_nsec()) * 1e-9
}

// Exact below 2^53: each 32-bit half converts without loss.
fn exact(value: i64) -> f64 {
    let high = i32::try_from(value >> 32).expect("the high half of an i64 fits an i32");
    let low = u32::try_from(value & 0xFFFF_FFFF).expect("the low half of an i64 fits a u32");
    f64::from(high) * 4_294_967_296.0 + f64::from(low)
}
