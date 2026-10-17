use std::path::Path;

use serde_json::{Value, json};
use syntax::Language;

use super::cases::{ROOT, cases, fixture_store, source_text, writer};
use super::indexed::{Cache, PYTHON, indexed, observed};
use crate::{FactsDb, Lookup, Source, open_facts};

#[test]
fn each_lookup_gives_the_reference_blocks_reason_or_miss() {
    let (cache, store) = fixture_store();
    for case in cases().lookups {
        let language = Language::from_name(&case.language).expect("a language name");
        let text = source_text(&case.path, &case.edit);
        let lookup = store.blocks_for(&case.path, &text, language);
        let outcome = shown(&lookup, &cache.database);
        assert_eq!(outcome, case.outcome, "{}", case.name);
    }
}

#[test]
fn a_null_metrics_column_or_an_error_with_no_clone_reason_is_a_miss() {
    let damages = [
        "UPDATE files SET metrics = NULL WHERE path = 'a.py'",
        "UPDATE files SET metrics = CAST('{\"Err\":{\"reason\":\"r\",\"blocks_reason\":null}}' \
         AS BLOB) WHERE path = 'a.py'",
    ];
    for damage in damages {
        assert_eq!(damaged_lookup(damage), json!(["miss"]), "{damage}");
    }
}

#[test]
fn a_metrics_cell_that_does_not_read_gives_the_warning() {
    let damages = [
        (
            "UPDATE files SET metrics = x'7b' WHERE path = 'a.py'",
            "JSONDecodeError('EOF while parsing an object at line 1 column 1')",
        ),
        (
            "UPDATE files SET metrics = 'text' WHERE path = 'a.py'",
            "TypeError(\"a stored blob holds 'text'\")",
        ),
    ];
    for (damage, cause) in damages {
        let warning = format!(
            "tag cache {{db}}: block rows for a.py are unreadable ({cause}); parsing the file \
             instead"
        );
        assert_eq!(damaged_lookup(damage), json!(["unreadable", warning]));
    }
}

// The snapshot holds the rows that the open saw: a later run that rewrites a.py's row changes
// nothing that the open lookup gives.
#[test]
fn a_lookup_reads_the_snapshot_of_its_open() {
    let cache = Cache::new();
    indexed(&cache.database, observed(&[("a.py", PYTHON)]), false);
    let store = usable(&cache.database);
    let before = store.blocks_for("a.py", PYTHON, Language::Python);

    let edited = format!("{PYTHON}\n\ndef other(y):\n    return y\n");
    indexed(&cache.database, observed(&[("a.py", &edited)]), false);

    let after = store.blocks_for("a.py", PYTHON, Language::Python);
    assert!(matches!(before, Lookup::Hit(_)), "{before:?}");
    assert_eq!(after, before);
    let fresh = usable(&cache.database);
    assert_eq!(
        fresh.blocks_for("a.py", PYTHON, Language::Python),
        Lookup::Miss
    );
}

fn usable(database: &Path) -> FactsDb {
    match open_facts(database, ROOT, false) {
        Source::Usable(db) => db,
        other => panic!("the database is not usable: {other:?}"),
    }
}

// The lookup of a.py in a store that holds it, after `damage`, in the fixture form.
fn damaged_lookup(damage: &str) -> Value {
    let cache = Cache::new();
    indexed(&cache.database, observed(&[("a.py", PYTHON)]), false);
    writer(&cache.database)
        .execute(damage, ())
        .unwrap_or_else(|error| panic!("{damage}: {error}"));
    let lookup = usable(&cache.database).blocks_for("a.py", PYTHON, Language::Python);
    shown(&lookup, &cache.database)
}

// The lookup in the form that the fixtures record, with the database path written {db}.
fn shown(lookup: &Lookup, database: &Path) -> Value {
    match lookup {
        Lookup::Hit(blocks) => {
            let rows: Vec<Value> = blocks
                .iter()
                .map(|block| {
                    json!([
                        block.start_line,
                        block.end_line,
                        block.key.to_string(),
                        block.exact.to_string(),
                        block.function,
                        block.tokens
                    ])
                })
                .collect();
            json!(["hit", rows])
        }
        Lookup::Unmeasured(reason) => json!(["unmeasured", reason]),
        Lookup::Miss => json!(["miss"]),
        Lookup::Unreadable(failure) => {
            let text = failure.to_string();
            let placed = text.replace(&database.display().to_string(), "{db}");
            json!(["unreadable", placed])
        }
    }
}
