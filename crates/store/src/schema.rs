//! The tables of agentless.db and the statements that read and write them.

// The blobs come last, and the references blob last of all, so that a read of an earlier
// column never loads the pages of a later one.
pub(crate) const CREATE: &str = "
CREATE TABLE IF NOT EXISTS meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    schema INTEGER NOT NULL,
    repo_root TEXT NOT NULL,
    generation TEXT NOT NULL,
    analysis BLOB NOT NULL,
    created INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    language TEXT NOT NULL,
    digest BLOB NOT NULL,
    tags INTEGER NOT NULL,
    imports INTEGER NOT NULL,
    refs INTEGER NOT NULL,
    metrics BLOB,
    symbols BLOB NOT NULL,
    facts BLOB NOT NULL
);
";

pub(crate) const DROP: &str = "
DROP TABLE IF EXISTS files;
DROP TABLE IF EXISTS meta;
";

// The shared meta reader takes the schema, the repository and the generation first.
pub(crate) const META: &str =
    "SELECT schema, repo_root, generation, analysis FROM meta WHERE id = 1";
pub(crate) const SCHEMA: &str = "SELECT schema FROM meta WHERE id = 1";
pub(crate) const ANALYSIS: &str = "SELECT analysis FROM meta WHERE id = 1";
pub(crate) const WRITE_META: &str = "INSERT INTO meta (id, schema, repo_root, generation, \
    analysis, created) VALUES (1, ?1, ?2, ?3, ?4, ?5)";
pub(crate) const CLEAR_META: &str = "DELETE FROM meta";

pub(crate) const ENTRIES: &str =
    "SELECT path, language, digest, metrics IS NOT NULL FROM files ORDER BY path";
pub(crate) const LOOKUP: &str = "SELECT id, language, digest FROM files WHERE path = ?1";
pub(crate) const SYMBOLS: &str = "SELECT symbols FROM files WHERE id = ?1";
pub(crate) const FACTS: &str = "SELECT symbols, facts FROM files WHERE id = ?1";
pub(crate) const METRICS: &str = "SELECT metrics FROM files WHERE id = ?1";
pub(crate) const ALL: &str = "SELECT symbols, facts, metrics FROM files WHERE id = ?1";
pub(crate) const COUNTS: &str = "SELECT COUNT(*), COALESCE(SUM(tags), 0), \
    COALESCE(SUM(imports), 0), COALESCE(SUM(refs), 0) FROM files";

pub(crate) const DELETE: &str = "DELETE FROM files WHERE path = ?1";
pub(crate) const INSERT: &str = "INSERT INTO files (path, language, digest, tags, imports, refs, \
    metrics, symbols, facts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";
