//! The shortest import chain from one file to another, over the edges a filter keeps, and the
//! statement that each resolved import row comes from.

use facts::Runs;

use super::rust::scanned;
use crate::{Scopes, SymbolGraph, import_chain};

// a.py imports b.py at load. b.py imports a.py in a function and c.py at load, and c.py imports
// a.py at load, so the direct edge back runs only when the function runs.
const FILES: [(&str, &str); 4] = [
    ("a.py", "import b\n"),
    ("b.py", "import c\n\n\ndef f():\n    import a\n"),
    ("c.py", "import a\nimport d\n"),
    ("d.py", "x = 1\n"),
];

// Each chain as (path, line, target) per hop.
fn hops(chain: Option<Vec<&crate::SymbolEdge<'_>>>) -> Option<Vec<(String, u32, String)>> {
    let rows = chain?.into_iter().map(|edge| {
        let (source, target) = (edge.source(), edge.target());
        (
            source.path().to_owned(),
            source.line(),
            target.path().to_owned(),
        )
    });
    Some(rows.collect())
}

fn hop(path: &str, line: u32, target: &str) -> (String, u32, String) {
    (path.to_owned(), line, target.to_owned())
}

#[test]
fn the_shortest_chain_takes_an_edge_that_the_filter_keeps_and_no_other() {
    let files = scanned(&FILES);
    let scopes = Scopes::build(&files);
    let graph = SymbolGraph::imports(&scopes);
    let every = hops(import_chain(&graph, "b.py", "a.py", |_, _| true));
    assert_eq!(every, Some(vec![hop("b.py", 5, "a.py")]));
    let at_load = |path: &str, line: u32| !(path == "b.py" && line == 5);
    let loaded = hops(import_chain(&graph, "b.py", "a.py", at_load));
    assert_eq!(
        loaded,
        Some(vec![hop("b.py", 1, "c.py"), hop("c.py", 1, "a.py")])
    );
    let no_c = |path: &str, line: u32| at_load(path, line) && path != "c.py";
    assert_eq!(hops(import_chain(&graph, "b.py", "a.py", no_c)), None);
    assert_eq!(
        hops(import_chain(&graph, "d.py", "a.py", |_, _| true)),
        None
    );
}

#[test]
fn each_resolved_row_names_the_position_of_its_statement() {
    let files = scanned(&[
        (
            "a.py",
            "import os\nfrom pkg import mod, name\n\n\ndef f():\n    import b\n",
        ),
        ("b.py", "x = 1\n"),
        ("pkg/__init__.py", "name = 1\n"),
        ("pkg/mod.py", "y = 2\n"),
    ]);
    let scopes = Scopes::build(&files);
    let scope = scopes.get("a.py").expect("a scanned file");
    let rows: Vec<(usize, &str, &str)> = scope.resolved_statements("a.py").collect();
    assert_eq!(
        rows,
        [
            (1, "pkg", "pkg/__init__.py"),
            (1, "pkg.mod", "pkg/mod.py"),
            (2, "b", "b.py"),
        ]
    );
    let runs: Vec<&Runs> = files[0]
        .facts
        .imports
        .iter()
        .map(|import| &import.runs)
        .collect();
    assert_eq!(runs, [&Runs::Load, &Runs::Load, &Runs::Call]);
}
