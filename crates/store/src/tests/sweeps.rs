use std::fs::{File, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use io::testing::Tree;

use crate::sweep::{cache_entries, evicted};
use crate::{CacheEntry, EvictionReport, enforce_cache_limit, evict_plan};

const NOW: f64 = 1_800_000_000.0;
const DAY: f64 = 86_400.0;

fn entry(name: &str, size: u64, used_at: f64) -> CacheEntry {
    CacheEntry {
        database: PathBuf::from(format!("/cache/{name}/agentless.db")),
        size,
        used_at,
    }
}

fn names(victims: &[CacheEntry]) -> Vec<String> {
    victims
        .iter()
        .map(|victim| {
            let directory = victim.database.parent().expect("a key directory");
            directory
                .file_name()
                .expect("a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

fn planned(entries: &[CacheEntry], limit: u64, keep: &str) -> Vec<String> {
    let keep = PathBuf::from(format!("/cache/{keep}/agentless.db"));
    names(&evict_plan(entries, limit, &keep, NOW))
}

#[test]
fn the_first_unprotected_entry_that_does_not_fit_goes_with_every_colder_one() {
    let entries = [
        entry("new", 10, NOW - 2.0 * DAY),
        entry("a", 50, NOW - 3.0 * DAY),
        entry("b", 1, NOW - 4.0 * DAY),
        entry("c", 1, NOW - 5.0 * DAY),
    ];
    assert_eq!(planned(&entries, 55, "none"), ["c", "b", "a"]);
    assert!(planned(&entries, 62, "none").is_empty());
}

#[test]
fn the_limit_test_is_strict() {
    let entries = [
        entry("new", 10, NOW - 2.0 * DAY),
        entry("a", 45, NOW - 3.0 * DAY),
    ];
    assert!(planned(&entries, 55, "none").is_empty());
    assert_eq!(planned(&entries, 54, "none"), ["a"]);
}

#[test]
fn protected_entries_count_toward_the_total_and_are_never_planned() {
    let entries = [
        entry("new", 10, NOW),
        entry("recent", 40, NOW - DAY),
        entry("cold", 10, NOW - 2.0 * DAY),
    ];
    assert_eq!(planned(&entries, 55, "none"), ["cold"]);
    assert!(planned(&entries, 0, "cold").is_empty());
    let boundary = [entry("new", 10, NOW), entry("edge", 10, NOW - DAY)];
    assert!(planned(&boundary, 0, "none").is_empty());
}

#[test]
fn the_newest_entry_and_the_kept_one_are_spared_whatever_their_size() {
    let entries = [
        entry("new", 100, NOW - 2.0 * DAY),
        entry("kept", 100, NOW - 3.0 * DAY),
        entry("old", 1, NOW - 4.0 * DAY),
    ];
    assert_eq!(planned(&entries, 10, "kept"), ["old"]);
}

// Equal times keep the listing order, as a stable sort does: the first listed is the warmer.
#[test]
fn equal_times_keep_the_listing_order() {
    let entries = [
        entry("new", 1, NOW - 2.0 * DAY),
        entry("x", 10, NOW - 3.0 * DAY),
        entry("y", 10, NOW - 3.0 * DAY),
    ];
    assert_eq!(planned(&entries, 15, "none"), ["y"]);
}

// A cache root of real files whose modification times are set.
struct Root {
    tree: Tree,
}

impl Root {
    fn new() -> Root {
        Root {
            tree: Tree::new().expect("a temporary directory"),
        }
    }

    fn path(&self) -> &Path {
        self.tree.path()
    }

    fn database(&self, key: &str, size: usize, used_at: f64) -> PathBuf {
        self.tree
            .create_dir(key)
            .expect("the key directory is created");
        let path = self
            .tree
            .write_file(format!("{key}/agentless.db"), vec![b'x'; size])
            .expect("the database is written");
        aged(&path, used_at);
        path
    }
}

fn aged(path: &Path, used_at: f64) {
    let when = UNIX_EPOCH + Duration::from_secs_f64(used_at);
    let times = FileTimes::new().set_accessed(when).set_modified(when);
    File::open(path)
        .and_then(|file| file.set_times(times))
        .expect("the times are set");
}

fn listing(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .expect("the directory lists")
        .map(|found| {
            found
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn the_sweep_deletes_the_cold_databases_and_their_sidecars_and_keeps_each_lock() {
    let root = Root::new();
    let keep = root.database("k", 100, NOW - 5.0 * DAY);
    root.database("new", 10, NOW - 2.0 * DAY);
    let cold = root.database("old", 30, NOW - 3.0 * DAY);
    root.tree
        .write_file("old/agentless.db-wal", vec![b'w'; 7])
        .expect("the sidecar is written");
    root.tree
        .write_file("old/agentless.lock", "")
        .expect("the lock file is written");

    let (report, warnings) = enforce_cache_limit(root.path(), &keep, Some(40), NOW);

    let freed = EvictionReport {
        databases: 1,
        size: 37,
    };
    assert_eq!((report, warnings), (freed, Vec::new()));
    assert!(!cold.exists());
    assert_eq!(listing(&root.path().join("old")), ["agentless.lock"]);
    assert!(keep.exists());
}

#[test]
fn a_locked_victim_is_kept_and_not_counted() {
    let root = Root::new();
    root.database("new", 10, NOW - 2.0 * DAY);
    let cold = root.database("old", 30, NOW - 3.0 * DAY);
    let held = io::try_lock(&cold.with_file_name("agentless.lock")).expect("the lock is free");

    let swept = enforce_cache_limit(root.path(), Path::new("/none"), Some(0), NOW);

    assert_eq!(swept, (EvictionReport::default(), Vec::new()));
    drop(held);
    assert!(cold.exists());
}

// The plan reads the times before any lock; the victim's time is read again under its lock.
#[test]
fn a_victim_used_after_the_plan_or_gone_before_its_turn_is_not_counted() {
    let root = Root::new();
    root.database("new", 10, NOW - 2.0 * DAY);
    let used = root.database("used", 30, NOW - 3.0 * DAY);
    let gone = root.database("gone", 30, NOW - 4.0 * DAY);
    let (entries, warnings) = cache_entries(root.path());
    assert!(warnings.is_empty());
    let victims = evict_plan(&entries, 0, Path::new("/none"), NOW);
    assert_eq!(names(&victims), ["gone", "used"]);
    aged(&used, NOW);
    io::remove_file(&gone).expect("the database is removed");

    let outcomes: Vec<_> = victims.iter().map(|victim| evicted(victim, NOW)).collect();

    assert_eq!(outcomes, [(None, Vec::new()), (None, Vec::new())]);
    assert!(used.exists());
}

#[test]
fn an_entry_that_cannot_be_weighed_is_named_and_left_out() {
    let root = Root::new();
    root.database("new", 10, NOW - 2.0 * DAY);
    root.tree
        .write_file("stray", "x")
        .expect("a stray file is written");

    let (entries, warnings) = cache_entries(root.path());

    assert_eq!(names(&entries), ["new"]);
    let expected = format!(
        "could not weigh {}/stray/agentless.db for the cache ceiling: NotADirectoryError(20, 'Not \
         a directory')",
        root.path().display()
    );
    assert_eq!(warnings, [expected]);
}

#[test]
fn a_cache_root_that_cannot_be_listed_evicts_nothing_and_says_so() {
    let root = Root::new();
    let missing = root.path().join("missing");

    let swept = enforce_cache_limit(&missing, Path::new("/none"), Some(0), NOW);

    let expected = format!(
        "cache root {} could not be listed, so nothing was evicted: FileNotFoundError(2, 'No such \
         file or directory')",
        missing.display()
    );
    assert_eq!(swept, (EvictionReport::default(), vec![expected]));
}

#[test]
fn no_limit_sweeps_nothing() {
    let root = Root::new();
    let cold = root.database("old", 30, NOW - 3.0 * DAY);
    root.database("new", 10, NOW - 2.0 * DAY);

    let swept = enforce_cache_limit(root.path(), Path::new("/none"), None, NOW);

    assert_eq!(swept, (EvictionReport::default(), Vec::new()));
    assert!(cold.exists());
}
