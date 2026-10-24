use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::Value;

use super::indexed::{Cache, indexed, observed};
use crate::{FactsDb, Source, open_facts};

pub(super) const ROOT: &str = "/agentless-store-fixture/repo";

// The recorded reference cases hold every section, one case per row, in these field orders.
#[derive(Deserialize)]
pub(super) struct Cases {
    pub(super) lookups: Vec<LookupCase>,
    pub(super) receipts: Vec<(String, String, String, String, bool)>,
    pub(super) keys: Vec<(String, String, String)>,
    pub(super) nogit: Vec<(Vec<ManifestRow>, String)>,
    pub(super) values: Vec<ValueCase>,
    pub(super) discards: Vec<(String, Vec<String>, Vec<String>)>,
}

// A walked path, with its size and its mtime in nanoseconds when the stat succeeded.
pub(super) type ManifestRow = (String, Option<(u64, i128)>);

#[derive(Deserialize)]
pub(super) struct ValueCase {
    pub(super) cell: Vec<Value>,
    pub(super) int: (String, Value),
    pub(super) text: String,
    pub(super) truthy: bool,
    pub(super) repr: String,
}

// A source file, its language, the edit of its text, and what the reference cache reader gave.
#[derive(Deserialize)]
pub(super) struct LookupCase {
    pub(super) name: String,
    pub(super) path: String,
    pub(super) language: String,
    pub(super) edit: String,
    pub(super) outcome: Value,
}

impl LookupCase {
    // A case of a source file as it is, which the fixture store indexes.
    pub(super) fn plain(&self) -> bool {
        self.name == self.path
    }
}

pub(super) fn cases() -> Cases {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cases.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

// The text of a fixture source file after the case's edit.
pub(super) fn source_text(path: &str, edit: &str) -> String {
    if edit == "empty" {
        return String::new();
    }
    let sources =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/characterization/fixtures");
    let text = std::fs::read_to_string(sources.join(path))
        .unwrap_or_else(|error| panic!("{path}: {error}"));
    if edit == "append" { text + "\n" } else { text }
}

// An agentless.db that the store's own plan, parse and write built from the source file of each
// plain lookup case, open for reading.
pub(super) fn fixture_store() -> (Cache, FactsDb) {
    let cache = Cache::new();
    let texts: Vec<(String, String)> = cases()
        .lookups
        .iter()
        .filter(|case| case.plain())
        .map(|case| (case.path.clone(), source_text(&case.path, "none")))
        .collect();
    let files: Vec<(&str, &str)> = texts
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    indexed(&cache.database, observed(&files), false);
    let db = match open_facts(&cache.database, ROOT, false) {
        Source::Usable(db) => db,
        other => panic!("the fixture store is not usable: {other:?}"),
    };
    (cache, db)
}

pub(super) fn writer(database: &Path) -> Connection {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Connection::open_with_flags(database, flags).expect("the test opens the database to write")
}
