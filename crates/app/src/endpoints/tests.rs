use super::endpoint;
use super::scanned::{LINKED, Scanned};
use crate::scan::RepoFacts;
use crate::symbol_graph::with_resolved;

// The node, label and message each endpoint text resolves to over `files`.
fn resolved(files: &[(&str, &str)], texts: &[&str]) -> Vec<(String, String, String)> {
    let scanned = Scanned::new(files);
    let loaded = RepoFacts::load(&scanned.services, &scanned.reading());
    let facts = loaded.value.unwrap_or_else(|error| panic!("{error}"));
    let resolved = with_resolved(&facts.files, None, |index, graph| {
        let at = |text: &&str| {
            let found = endpoint(index, graph, text);
            (found.node, found.label, found.message)
        };
        Ok(texts.iter().map(at).collect())
    });
    resolved.unwrap_or_else(|error| panic!("{error}"))
}

fn row(node: &str, label: &str, message: &str) -> (String, String, String) {
    (node.to_owned(), label.to_owned(), message.to_owned())
}

#[test]
fn a_stable_id_a_path_and_a_unique_name_resolve_exactly() {
    let texts = ["py:pkg/money.py::format_money", "pkg/report.py", "render"];
    assert_eq!(
        resolved(&LINKED, &texts),
        [
            row("py:pkg/money.py::format_money", "format_money", ""),
            row("pkg/report.py", "pkg/report.py", ""),
            row("py:pkg/report.py::render", "render", ""),
        ]
    );
}

#[test]
fn an_unknown_name_is_refused_with_the_text_as_its_label() {
    let found = resolved(&LINKED, &["nothing_here"]);
    assert_eq!(
        found,
        [row(
            "",
            "nothing_here",
            "no symbol or file matches nothing_here"
        )]
    );
}

#[test]
fn several_definitions_of_equal_standing_are_ambiguous() {
    let files = [
        ("a.py", "def run():\n    return 1\n"),
        ("b.py", "def run():\n    return 2\n"),
    ];
    let found = resolved(&files, &["run"]);
    let message = "run is ambiguous: py:a.py::run, py:b.py::run";
    assert_eq!(found, [row("", "run", message)]);
}

#[test]
fn an_exactly_spelled_qualified_name_outranks_a_suffix_match() {
    let files = [
        ("a.py", "def run():\n    return 1\n"),
        ("b.py", "class Job:\n    def run(self):\n        return 2\n"),
    ];
    let found = resolved(&files, &["Job.run"]);
    assert_eq!(found, [row("py:b.py::Job.run", "Job.run", "")]);
}

#[test]
fn a_file_qualified_endpoint_never_moves_to_another_file() {
    let texts = [
        "py:pkg/gone.py::format_money",
        "pkg/money.py::format_money",
        "pkg/money.py:2",
    ];
    let moved = "no symbol or file matches py:pkg/gone.py::format_money: the scan read no file \
                 pkg/gone.py. 1 definition of format_money: py:pkg/money.py::format_money. Pass \
                 it as the target.";
    assert_eq!(
        resolved(&LINKED, &texts),
        [
            row("", "py:pkg/gone.py::format_money", moved),
            row("py:pkg/money.py::format_money", "format_money", ""),
            row("py:pkg/money.py::format_money", "format_money", ""),
        ]
    );
}
