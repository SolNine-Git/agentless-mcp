use std::os::unix::fs::PermissionsExt;

use super::cases::writer;
use super::indexed::{Cache, PYTHON, indexed, observed};
use crate::{FACTS_SCHEMA_VERSION, WriteError, Writer, analysis};

#[test]
fn a_held_lock_refuses_the_writer_and_names_its_directory() {
    let cache = Cache::new();
    indexed(&cache.database, observed(&[("a.py", PYTHON)]), false);
    let directory = cache.database.parent().expect("a key directory");
    let held = io::try_lock(&directory.join("agentless.lock")).expect("the lock is free");

    let (opened, warnings) = Writer::open(&cache.database);

    assert!(warnings.is_empty());
    let refused = opened.expect_err("the lock is held");
    assert!(
        matches!(&refused, WriteError::Locked { directory: found } if found == directory),
        "{refused:?}"
    );
    drop(held);
}

#[test]
fn the_writer_creates_the_directory_owner_only() {
    let cache = Cache::new();

    let (opened, warnings) = Writer::open(&cache.database);

    assert!(opened.is_ok() && warnings.is_empty());
    let directory = cache.database.parent().expect("a key directory");
    for level in [directory, directory.parent().expect("the cache root")] {
        let mode = std::fs::metadata(level)
            .expect("the level exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "{}", level.display());
    }
}

// The writer restores the directory's mode itself, so a directory in the lock file's place is
// what makes the lock open fail.
#[test]
fn a_lock_file_that_cannot_be_opened_is_an_io_error() {
    let cache = Cache::new();
    cache
        .tree
        .create_dir("cache/key/agentless.lock")
        .expect("the directory is created");

    let (opened, _) = Writer::open(&cache.database);

    assert!(matches!(opened, Err(WriteError::Io(_))), "{opened:?}");
}

#[test]
fn the_previous_rows_are_the_written_ones_under_this_analysis() {
    let cache = Cache::new();
    indexed(
        &cache.database,
        observed(&[("b.py", PYTHON), ("a.py", "x = 1\n")]),
        false,
    );

    let (opened, _) = Writer::open(&cache.database);
    let writer_open = opened.expect("the writer opens");
    let previous = writer_open.previous();

    let paths: Vec<&str> = previous.entries.keys().map(String::as_str).collect();
    assert_eq!(paths, ["a.py", "b.py"]);
    assert_eq!(
        previous.analysis.as_deref(),
        Some(analysis().bytes().as_slice())
    );
    let entry = &previous.entries["a.py"];
    assert_eq!((entry.language.as_str(), entry.measured), ("python", true));
    assert_eq!(entry.digest.len(), 32);
}

// The rows go in in path order, so their ids follow the paths.
#[test]
fn the_rows_are_written_in_path_order() {
    let cache = Cache::new();

    indexed(
        &cache.database,
        observed(&[("a.py", PYTHON), ("c.py", PYTHON), ("b.py", PYTHON)]),
        false,
    );

    let connection = writer(&cache.database);
    let mut statement = connection
        .prepare("SELECT path FROM files ORDER BY id")
        .expect("the query prepares");
    let paths: Vec<String> = statement
        .query_map((), |row| row.get(0))
        .and_then(Iterator::collect)
        .expect("the rows read");
    assert_eq!(paths, ["a.py", "b.py", "c.py"]);
}

// A database of another layout loses its tables, so nothing of it is reused.
#[test]
fn a_database_of_another_schema_is_migrated_and_starts_empty() {
    let cache = Cache::new();
    indexed(&cache.database, observed(&[("a.py", PYTHON)]), false);
    writer(&cache.database)
        .execute("UPDATE meta SET schema = 0", ())
        .expect("the damage applies");

    let report = indexed(&cache.database, observed(&[("a.py", PYTHON)]), false);

    assert_eq!((report.indexed, report.reused, report.files), (1, 0, 1));
    let schema: i64 = writer(&cache.database)
        .query_row("SELECT schema FROM meta", (), |row| row.get(0))
        .expect("the meta row reads");
    assert_eq!(schema, i64::from(FACTS_SCHEMA_VERSION));
}
