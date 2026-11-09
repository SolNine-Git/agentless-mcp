use std::sync::Arc;

use syntax::{Language, parse};

use crate::measure;

// Each unit's qualname with its action names, from one parse of `source`.
fn actions_of(language: Language, source: &str) -> Vec<(String, Vec<String>)> {
    let parsed = parse(language, Arc::from(source)).expect("a tree");
    let measured = measure(&parsed).expect("it measures");
    assert_eq!(measured.actions.len(), measured.units.len());
    let units = measured.units.into_iter().map(|unit| unit.qualname);
    units.zip(measured.actions).collect()
}

fn row(qualname: &str, names: &[&str]) -> (String, Vec<String>) {
    let names = names.iter().map(|name| (*name).to_owned()).collect();
    (qualname.to_owned(), names)
}

const RUST: &str = r#"impl Store {
    pub fn load(&self, path: &Path) -> Option<String> {
        // read_all(path) is a comment
        let text = std::fs::read_to_string(path).ok()?;
        let label = "skip(this)";
        self.cache.insert(path.to_owned(), text.clone());
        Some(text.trim().to_owned())
    }
}

fn walk(depth: u32) -> u32 {
    if depth == 0 { 0 } else { walk(depth - 1) + COUNT }
}

fn render(items: &[Item]) -> String {
    let lines: Vec<String> = items.iter().map(|item| item.label.clone()).collect::<Vec<_>>();
    format!("{}", lines.join(", "))
}
"#;

// `collect::<Vec<_>>()` puts `::` after the name, so the rule reads `collect` as a field.
#[test]
fn a_rust_unit_names_its_calls_and_its_field_reads() {
    let found = actions_of(Language::Rust, RUST);
    let expected = [
        row(
            "Store.load",
            &[
                "Some",
                "cache",
                "clone",
                "insert",
                "ok",
                "read_to_string",
                "to_owned",
                "trim",
            ],
        ),
        row("walk", &[]),
        row(
            "render",
            &["clone", "collect", "iter", "join", "label", "map"],
        ),
    ];
    assert_eq!(found, expected);
}

const PYTHON: &str = r#"class Reader:
    def read(self, path):
        """Read path(s) and parse(them)."""
        with open(path) as handle:  # close(handle) is a comment
            rows = json.load(handle)
        self.count += len(rows)
        title = f"{self.title(path)}"

        def keep(row):
            return row.name.startswith(PREFIX)

        return [row.name for row in rows if keep(row) and self.read(row)]
"#;

#[test]
fn a_python_unit_names_its_calls_and_its_field_reads_and_a_nested_unit_has_its_own() {
    let found = actions_of(Language::Python, PYTHON);
    let expected = [
        row(
            "Reader.read",
            &["count", "keep", "len", "load", "name", "open", "startswith"],
        ),
        row("Reader.read.keep", &["name", "startswith"]),
    ];
    assert_eq!(found, expected);
}
