use std::path::{Path, PathBuf};
use std::time::Duration;

use io::testing::{Tree, latin};
use rusqlite::{Connection, OpenFlags, named_params};
use serde::Deserialize;
use serde_json::Value;

use crate::Source;

pub(super) const ROOT: &str = "/agentless-store-fixture/repo";
pub(super) const BUSY: Duration = Duration::from_secs(5);
const DATABASE: &str = "tags.db";

// scripts/store_fixtures.py writes every section, one case per row, in these field orders.
#[derive(Deserialize)]
pub(super) struct Cases {
    pub(super) fixture: Vec<(String, [u64; 4])>,
    pub(super) opens: Vec<OpenCase>,
    pub(super) lookups: Vec<LookupCase>,
    pub(super) receipts: Vec<(String, String, String, String, bool)>,
    pub(super) keys: Vec<(String, String, String)>,
    pub(super) nogit: Vec<(Vec<ManifestRow>, String)>,
    pub(super) values: Vec<ValueCase>,
    pub(super) sloc: Vec<(Vec<Value>, Option<String>)>,
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

#[derive(Deserialize)]
pub(super) struct OpenCase {
    pub(super) name: String,
    pub(super) damage: Vec<Value>,
    pub(super) repo_generation: String,
    pub(super) kind: String,
    pub(super) receipt: String,
    pub(super) note: String,
    pub(super) exists: bool,
}

#[derive(Deserialize)]
pub(super) struct LookupCase {
    pub(super) name: String,
    pub(super) path: String,
    pub(super) language: String,
    pub(super) edit: String,
    pub(super) damage: Vec<Value>,
    pub(super) outcome: Value,
}

pub(super) fn cases() -> Cases {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/cases.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

pub(super) fn fixture_database() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(DATABASE)
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

pub(super) fn usable(source: Source, name: &str) -> crate::TagsDb {
    match source {
        Source::Usable(opened) => opened,
        other => panic!("{name}: the fixture copy is not usable: {other:?}"),
    }
}

// A copy of the fixture database in its own temporary directory.
pub(super) struct Scratch {
    pub(super) tree: Tree,
    pub(super) database: PathBuf,
}

impl Scratch {
    pub(super) fn new() -> Self {
        let tree = Tree::new().expect("the temporary directory takes a tree");
        let database = tree
            .copy_file(fixture_database(), DATABASE)
            .expect("the fixture database is copied");
        Self { tree, database }
    }

    // Applies one damage of the fixtures; `path` names the file that a statement's :file_id binds.
    pub(super) fn damage(&self, damage: &[Value], path: &str) {
        let argument = damage.get(1);
        match damage[0].as_str().expect("a damage kind") {
            "sql" => {
                let statements = argument.and_then(Value::as_array).expect("statements");
                self.run_sql(statements, path);
            }
            "bytes" => self.write(&latin(argument.and_then(Value::as_str).expect("bytes"))),
            "truncate" => self.truncate(argument.and_then(Value::as_u64).expect("a length")),
            "directory" => self.replace_with_directory(),
            "missing" => self.remove(),
            "none" | "bypass" | "locked" => {}
            other => panic!("unknown damage kind {other}"),
        }
    }

    fn run_sql(&self, statements: &[Value], path: &str) {
        let connection = writer(&self.database);
        for statement in statements {
            let statement = statement.as_str().expect("a statement");
            let done = if statement.contains(":file_id") {
                let file_id = file_id(&connection, path);
                connection.execute(statement, named_params! {":file_id": file_id})
            } else {
                connection.execute(statement, ())
            };
            done.unwrap_or_else(|error| panic!("{statement}: {error}"));
        }
    }

    fn write(&self, bytes: &[u8]) {
        self.tree
            .write_file(DATABASE, bytes)
            .expect("the damaged database is written");
    }

    fn truncate(&self, length: u64) {
        let bytes = std::fs::read(fixture_database()).expect("the fixture database is read");
        let length = usize::try_from(length).expect("a length that fits");
        self.write(&bytes[..length]);
    }

    fn remove(&self) {
        assert!(io::remove_file(&self.database).expect("the database is removed"));
    }

    fn replace_with_directory(&self) {
        self.remove();
        self.tree
            .create_dir(DATABASE)
            .expect("a directory takes the database path");
    }
}

fn file_id(connection: &Connection, path: &str) -> i64 {
    let sql = "SELECT id FROM files WHERE path = ?";
    connection
        .query_row(sql, [path], |row| row.get(0))
        .expect("the damaged file is indexed")
}

pub(super) fn writer(database: &Path) -> Connection {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Connection::open_with_flags(database, flags).expect("the test opens the database to write")
}

// A writer in exclusive locking mode keeps its lock after its commit.
pub(super) fn exclusive_writer(database: &Path) -> Connection {
    let connection = writer(database);
    connection
        .execute_batch(
            "PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE; \
             UPDATE meta SET created_at = created_at WHERE id = 1; COMMIT",
        )
        .expect("the writer takes the exclusive lock");
    connection
}
