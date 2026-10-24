// An agentless.db built by the store's own plan, parse and write, for the tests that read it.

use std::path::{Path, PathBuf};

use io::testing::Tree;
use syntax::Language;

use super::cases::ROOT;
use crate::{
    EvictionReport, FileText, IndexReport, NewMeta, Observed, Read, Writer, analysis, analyze,
    encode, plan, settle,
};

pub(super) const GENERATION: &str = "0123abcd";

pub(super) const PYTHON: &str =
    "import os\n\n\nclass Box:\n    def size(self, x):\n        return os.sep + x\n";

pub(super) fn file(path: &str, text: &str) -> FileText {
    let language = Language::from_path(path).expect("a known language");
    FileText::decode(path.to_owned(), language, text.as_bytes())
}

// A temporary directory with the database path of one repository in it.
pub(super) struct Cache {
    pub(super) tree: Tree,
    pub(super) database: PathBuf,
}

impl Cache {
    pub(super) fn new() -> Cache {
        let tree = Tree::new().expect("a temporary directory");
        let database = tree.join("cache/key/agentless.db");
        Cache { tree, database }
    }
}

// Each file is measured when its language has metrics rules, as the index of a source file is.
pub(super) fn observed(files: &[(&str, &str)]) -> Vec<Observed> {
    files
        .iter()
        .map(|(path, text)| {
            let file = file(path, text);
            Observed {
                path: (*path).to_owned(),
                language: file.language,
                measure: metrics::stamp(file.language).is_some(),
                read: Read::Text(file),
            }
        })
        .collect()
}

// Indexes `observed` into `database` with one serial parse and encode of each planned file.
pub(super) fn indexed(database: &Path, observed: Vec<Observed>, force: bool) -> IndexReport {
    let (writer, warnings) = Writer::open(database);
    assert!(warnings.is_empty(), "{warnings:?}");
    let writer = writer.expect("the writer opens");
    let plan = plan(writer.previous(), observed, analysis(), force);
    let parsed = plan
        .extract
        .iter()
        .map(|planned| analyze(&planned.file, planned.measure).map(|parsed| encode(&parsed)))
        .collect();
    let settled = settle(plan, parsed);
    let meta = NewMeta {
        repo_root: ROOT,
        generation: GENERATION,
        created: 0,
    };
    let counts = writer.write(&settled, meta).expect("the write succeeds");
    let database = database.to_path_buf();
    settled.report(
        database,
        GENERATION.to_owned(),
        counts,
        EvictionReport::default(),
    )
}
