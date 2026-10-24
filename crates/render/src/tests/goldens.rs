use std::path::{Path, PathBuf};

use io::ProjectConfig;
use serde_json::Value;

use super::rows::report;
use crate::{Receipt, lint_json, lint_text};

// The characterization goldens of the Python lint report, read where they are committed.
fn golden(name: &str) -> String {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "../../tests/characterization/goldens/lint",
        name,
    ]
    .iter()
    .collect();
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn field<'a>(fields: &'a Value, key: &str) -> &'a str {
    let text = fields[key].as_str();
    text.unwrap_or_else(|| panic!("{key}: no text in {fields}"))
}

// The JSON golden holds the report and the receipt, so it is the input for both goldens.
#[test]
fn the_lint_goldens_come_out_byte_for_byte() {
    let json = golden("repo_py.lint.json");
    let document: Value = serde_json::from_str(&json).unwrap_or_else(|error| panic!("{error}"));
    let candidates = document["candidates"].as_array();
    let report = report(candidates.unwrap_or_else(|| panic!("no candidates")));
    let fields = &document["receipt"];
    let dirty = fields["dirty"].as_u64().map(usize::try_from);
    let config = ProjectConfig::default();
    let receipt = Receipt {
        root: Path::new(field(fields, "repo")),
        head: fields["head"].as_str(),
        tree: fields["tree"].as_str(),
        dirty: dirty.map(|count| count.unwrap_or_else(|error| panic!("dirty: {error}"))),
        cache: field(fields, "cache"),
        note: field(fields, "note"),
        config: &config,
        warnings: Vec::new(),
        counter: &app::Chars4,
    };

    assert_eq!(lint_text(&receipt, &report), golden("repo_py.lint.txt"));
    let rendered = lint_json(&receipt, &report).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(rendered, json);
}
