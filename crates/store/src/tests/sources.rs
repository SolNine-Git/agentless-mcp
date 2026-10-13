use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rusqlite::params;

use super::cases::{ROOT, writer};
use super::indexed::{Cache, GENERATION, PYTHON, file, indexed, observed};
use crate::reader::open_facts_within;
use crate::{
    Discarded, FactSource, FactsDb, Freshness, IndexMeta, Refusal, Source, SqlClass, SqlError,
    analyze, discard_facts, freshness, open_facts,
};

const SCRIPT: &str = "export function add(a, b) {\n  return a + b;\n}\n";

fn usable(database: &Path) -> FactsDb {
    match open_facts(database, ROOT, false) {
        Source::Usable(db) => db,
        other => panic!("the database is not usable: {other:?}"),
    }
}

// A source over `db` whose parse counts its calls.
fn counting(db: Option<FactsDb>) -> (FactSource, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let parse = move |language, text| {
        counted.fetch_add(1, Ordering::SeqCst);
        syntax::parse(language, text)
    };
    (FactSource::with(db, Arc::new(parse)), calls)
}

fn built(cache: &Cache) {
    indexed(
        &cache.database,
        observed(&[("a.py", PYTHON), ("b.js", SCRIPT)]),
        false,
    );
}

#[test]
fn a_row_that_describes_the_text_is_served_without_a_parse() {
    let cache = Cache::new();
    built(&cache);
    let (source, calls) = counting(Some(usable(&cache.database)));
    let text = file("a.py", PYTHON);
    let parsed = analyze(&text, true).expect("the file parses");

    let facts = source.facts(&text);
    let symbols = source.symbols(&text);
    let metrics = source.metrics(&text);
    let both = source.facts_and_metrics(&text);

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(facts.value.as_ref(), Ok(&parsed.facts));
    assert_eq!(symbols.value.as_ref(), Ok(&parsed.facts.symbols));
    assert_eq!(Some(&metrics.value), parsed.metrics.as_ref());
    let measured = parsed.metrics.clone().expect("measured");
    assert_eq!(both.value, Ok((parsed.facts, measured)));
    assert!(
        [
            facts.warning,
            symbols.warning,
            both.warning,
            metrics.warning
        ]
        .iter()
        .all(Option::is_none)
    );
}

// One parse fills both values on a miss, and each other read of a miss parses once.
#[test]
fn a_miss_parses_once_for_each_read() {
    let cache = Cache::new();
    built(&cache);
    let (source, calls) = counting(Some(usable(&cache.database)));
    let edited = file("a.py", &format!("{PYTHON}y = 2\n"));
    let parsed = analyze(&edited, true).expect("the file parses");

    let both = source.facts_and_metrics(&edited);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let measured = parsed.metrics.clone().expect("measured");
    assert_eq!(both.value, Ok((parsed.facts.clone(), measured)));

    let facts = source.facts(&edited);
    let metrics = source.metrics(&edited);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(facts.value, Ok(parsed.facts));
    assert_eq!(Some(metrics.value), parsed.metrics);
}

#[test]
fn without_a_database_every_read_parses() {
    let (source, calls) = counting(None);
    let text = file("b.js", SCRIPT);

    let facts = source.facts(&text);

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let parsed = analyze(&text, false).expect("the file parses");
    assert_eq!(facts.value, Ok(parsed.facts));
    assert!(FactSource::parsing().database().is_none());
}

// A file indexed outside the metric scope has facts and no metrics: its metrics read parses.
#[test]
fn an_unmeasured_row_serves_facts_and_parses_for_metrics() {
    let cache = Cache::new();
    let mut files = observed(&[("tests/a.py", PYTHON)]);
    files[0].measure = false;
    indexed(&cache.database, files, false);
    let (source, calls) = counting(Some(usable(&cache.database)));
    let text = file("tests/a.py", PYTHON);

    let facts = source.facts(&text);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let metrics = source.metrics(&text);
    let both = source.facts_and_metrics(&text);

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(facts.value.is_ok() && metrics.value.is_ok() && both.value.is_ok());
}

// The references blob is read only by the full facts read, so its damage spares the symbols.
#[test]
fn a_bad_blob_gives_the_warning_and_a_parse_and_spares_the_symbols() {
    let cache = Cache::new();
    built(&cache);
    let damage = "UPDATE files SET facts = x'7b' WHERE path = 'a.py'";
    writer(&cache.database)
        .execute(damage, ())
        .expect("the damage applies");
    let (source, calls) = counting(Some(usable(&cache.database)));
    let text = file("a.py", PYTHON);

    let facts = source.facts(&text);
    let symbols = source.symbols(&text);

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        facts.value,
        analyze(&text, false).map(|parsed| parsed.facts)
    );
    let shown = facts.warning.expect("a warning").to_string();
    let expected = format!(
        "tag cache {}: fact rows for a.py are unreadable (JSONDecodeError('EOF while parsing an \
         object at line 1 column 1')); parsing the file instead",
        cache.database.display()
    );
    assert_eq!(shown, expected);
    assert!(symbols.warning.is_none() && symbols.value.is_ok());
}

#[test]
fn a_row_of_another_analysis_or_language_is_a_miss() {
    for damage in [
        "UPDATE meta SET analysis = x'00'",
        "UPDATE files SET language = 'go' WHERE path = 'a.py'",
        "UPDATE files SET digest = x'00' WHERE path = 'a.py'",
    ] {
        let cache = Cache::new();
        built(&cache);
        writer(&cache.database)
            .execute(damage, ())
            .expect("the damage applies");
        let (source, calls) = counting(Some(usable(&cache.database)));

        let served = source.symbols(&file("a.py", PYTHON));

        assert_eq!(calls.load(Ordering::SeqCst), 1, "{damage}");
        assert!(served.warning.is_none() && served.value.is_ok(), "{damage}");
    }
}

#[test]
fn the_database_counts_its_files_and_their_facts_and_names_its_generation() {
    let cache = Cache::new();
    let report = indexed(
        &cache.database,
        observed(&[("a.py", PYTHON), ("b.js", SCRIPT)]),
        false,
    );
    let db = usable(&cache.database);

    let counts = db.row_counts().expect("the counts read");

    assert_eq!(db.generation(), GENERATION);
    assert_eq!(counts.files, 2);
    assert_eq!(
        (counts.tags, counts.imports, counts.refs),
        (report.tags, report.imports, report.refs)
    );
    assert_eq!((counts.tags, counts.imports), (3, 1));
}

fn refusal(database: &Path) -> Refusal {
    match open_facts(database, ROOT, false) {
        Source::Refused(refusal) => refusal,
        other => panic!("the database is not refused: {other:?}"),
    }
}

#[test]
fn each_unusable_database_is_refused_with_its_note() {
    let cases = [
        (
            "DELETE FROM meta",
            "cache discarded: no index has been built",
        ),
        (
            "UPDATE meta SET schema = 2",
            "cache discarded: schema 2 predates 1, rerun agentless-mcp index",
        ),
        (
            "UPDATE meta SET repo_root = '/elsewhere/repo'",
            "cache discarded: it was built for /elsewhere/repo",
        ),
    ];
    for (damage, note) in cases {
        let cache = Cache::new();
        built(&cache);
        writer(&cache.database)
            .execute(damage, ())
            .expect("the damage applies");

        let refused = refusal(&cache.database);

        assert_eq!(refused.to_string(), note, "{damage}");
        assert!(refused.discards(), "{damage}");
    }
}

#[test]
fn a_garbage_file_is_refused_as_corrupt_and_a_missing_one_is_absent() {
    let cache = Cache::new();
    assert!(matches!(
        open_facts(&cache.database, ROOT, false),
        Source::Absent
    ));
    assert!(matches!(
        open_facts(&cache.database, ROOT, true),
        Source::Bypassed
    ));
    let garbage = "this file is not a SQLite database\n".repeat(128);
    cache
        .tree
        .create_dir("cache/key")
        .and_then(|_| cache.tree.write_file("cache/key/agentless.db", garbage))
        .expect("the garbage is written");

    let refused = refusal(&cache.database);

    let error = SqlError {
        class: SqlClass::Database,
        message: "file is not a database".to_owned(),
    };
    assert_eq!(refused, Refusal::Corrupt(error));
}

// A writer that holds the database's lock past its wait makes the open unreadable, not corrupt.
#[test]
fn a_locked_database_is_unreadable_and_stays() {
    let cache = Cache::new();
    built(&cache);
    let holder = writer(&cache.database);
    holder
        .execute_batch(
            "PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; UPDATE meta SET created = 1; COMMIT",
        )
        .expect("the writer takes the exclusive lock");

    let found = open_facts_within(&cache.database, ROOT, Duration::ZERO);

    assert!(
        matches!(&found, Source::Refused(refusal) if !refusal.discards()),
        "{found:?}"
    );
    drop(holder);
    assert!(cache.database.exists());
}

#[test]
fn the_discard_removes_the_database_and_its_sidecars_and_keeps_the_lock() {
    let cache = Cache::new();
    built(&cache);
    for sidecar in ["-wal", "-shm"] {
        let name = format!("cache/key/agentless.db{sidecar}");
        cache
            .tree
            .write_file(&name, "")
            .expect("the sidecar is written");
    }
    let lock = cache.database.with_file_name("agentless.lock");
    let held = io::try_lock(&lock).expect("the lock is free");
    assert!(matches!(
        discard_facts(&cache.database),
        Discarded::LeftInPlace
    ));
    drop(held);

    assert!(matches!(discard_facts(&cache.database), Discarded::Removed));

    let left: Vec<String> = std::fs::read_dir(cache.database.parent().expect("a directory"))
        .expect("the directory lists")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(left, ["agentless.lock"]);
}

#[test]
fn freshness_refuses_in_the_reference_order_then_compares_the_generation() {
    let meta = |schema: i64, root: &str| IndexMeta {
        schema,
        repo_root: root.to_owned(),
        generation: GENERATION.to_owned(),
    };
    let found = [
        freshness(None, ROOT, GENERATION),
        freshness(Some(&meta(2, "/elsewhere")), ROOT, GENERATION),
        freshness(Some(&meta(1, "/elsewhere")), ROOT, GENERATION),
        freshness(Some(&meta(1, ROOT)), ROOT, GENERATION),
        freshness(Some(&meta(1, ROOT)), ROOT, "fedcba98"),
    ];
    let expected = [
        Freshness::Refused(Refusal::NoIndex),
        Freshness::Refused(Refusal::Schema {
            found: 2,
            expected: 1,
        }),
        Freshness::Refused(Refusal::OtherRepository {
            repo_root: "/elsewhere".to_owned(),
        }),
        Freshness::Current,
        Freshness::Stale,
    ];
    assert_eq!(found, expected);
}

#[test]
fn the_read_source_and_its_database_move_and_share_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<FactSource>();
    shared::<FactsDb>();
}

#[test]
fn a_row_counts_one_symbol_blob_one_facts_blob_and_its_counts() {
    let cache = Cache::new();
    built(&cache);
    let row: (i64, i64, i64) = writer(&cache.database)
        .query_row(
            "SELECT tags, imports, refs FROM files WHERE path = ?1",
            params!["a.py"],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("the row reads");
    let parsed = analyze(&file("a.py", PYTHON), false).expect("the file parses");
    let length = |count: usize| i64::try_from(count).expect("a small count");
    let expected = (
        length(parsed.facts.symbols.len()),
        length(parsed.facts.imports.len()),
        length(parsed.facts.refs.len()),
    );
    assert_eq!(row, expected);
}
