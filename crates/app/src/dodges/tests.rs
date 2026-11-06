use std::sync::Arc;

use graph::FileFacts;
use syntax::Language;

use super::import_facts;

fn scanned(path: &str, text: &str) -> FileFacts {
    let parsed = syntax::parse(Language::Python, Arc::from(text)).expect("a Python text parses");
    FileFacts {
        path: path.to_owned(),
        language: Language::Python,
        line_count: 2,
        facts: facts::extract(&parsed),
    }
}

// The import graph reads the symbols of a package entry, so they stay outside the dodge scope too.
#[test]
fn a_package_entry_keeps_its_symbols_outside_the_dodge_scope() {
    let symbols = |path: &str| {
        let file = scanned(path, "def mod():\n    pass\n");
        import_facts(&file).facts.symbols.len()
    };
    assert_eq!(symbols("tests/pkg/__init__.py"), 1);
    assert_eq!(symbols("tests/pkg/a.py"), 0);
    assert_eq!(symbols("pkg/a.py"), 1);
}
