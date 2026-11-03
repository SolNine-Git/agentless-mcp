use graph::SymbolGraph;

use super::scanned::{LINKED, Scanned};
use super::{
    PATH_SOURCE_WORDING, PATH_TARGET_WORDING, Scope, TARGET_WORDING, Wording, bind, endpoint,
};
use crate::scan::RepoFacts;
use crate::symbol_graph::with_resolved;

// The texts and the answers of these tests spell the repository root this way.
const ROOT: &str = "<ROOT>";

// The fields that `each` gives for each text over the scope and the graph of `files`.
fn each_over(
    files: &[(&str, &str)],
    texts: &[&str],
    each: impl Fn(&Scope<'_, '_>, &SymbolGraph<'_>, &str) -> Vec<String>,
) -> Vec<Vec<String>> {
    let scanned = Scanned::new(files);
    let reading = scanned.reading();
    let root = reading.repo.root.display().to_string();
    let loaded = RepoFacts::load(&scanned.services, &reading);
    let facts = loaded.value.unwrap_or_else(|error| panic!("{error}"));
    let answered = with_resolved(&facts.files, None, |index, graph| {
        let scope = Scope {
            index,
            skipped: &facts.skipped,
            root: &reading.repo.root,
        };
        let at = |text: &&str| {
            let fields = each(&scope, graph, &text.replace(ROOT, &root));
            (fields.into_iter())
                .map(|field| field.replace(&root, ROOT))
                .collect()
        };
        Ok(texts.iter().map(at).collect())
    });
    answered.unwrap_or_else(|error| panic!("{error}"))
}

// The node, label and message each endpoint text resolves to over `files`.
fn resolved_as(files: &[(&str, &str)], texts: &[&str], wording: &Wording) -> Vec<Vec<String>> {
    each_over(files, texts, |scope, graph, text| {
        let found = endpoint(scope, graph, text, wording);
        vec![found.node, found.label, found.message]
    })
}

fn resolved(files: &[(&str, &str)], texts: &[&str]) -> Vec<Vec<String>> {
    resolved_as(files, texts, &PATH_TARGET_WORDING)
}

fn row(node: &str, label: &str, message: &str) -> Vec<String> {
    vec![node.to_owned(), label.to_owned(), message.to_owned()]
}

// The status and the message that each text binds to as refs and explain bind it.
fn bound(files: &[(&str, &str)], texts: &[&str], limit: usize) -> Vec<Vec<String>> {
    each_over(files, texts, |scope, _, text| {
        let binding = bind(scope, text, limit, &TARGET_WORDING).binding;
        vec![binding.status.as_str().to_owned(), binding.message]
    })
}

fn status(status: &str, message: &str) -> Vec<String> {
    vec![status.to_owned(), message.to_owned()]
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

#[test]
fn a_source_endpoint_that_binds_to_nothing_says_to_pass_a_candidate_as_the_source() {
    let found = resolved_as(
        &LINKED,
        &["py:pkg/gone.py::format_money"],
        &PATH_SOURCE_WORDING,
    );
    let message = "no symbol or file matches py:pkg/gone.py::format_money: the scan read no file \
                   pkg/gone.py. 1 definition of format_money: py:pkg/money.py::format_money. \
                   Pass it as the source.";
    assert_eq!(found, [row("", "py:pkg/gone.py::format_money", message)]);
}

#[test]
fn an_endpoint_written_from_the_root_or_as_an_absolute_path_below_it_resolves() {
    let texts = [
        "./pkg/report.py",
        "<ROOT>/pkg/money.py::format_money",
        "./pkg/money.py:2",
    ];
    assert_eq!(
        resolved(&LINKED, &texts),
        [
            row("pkg/report.py", "pkg/report.py", ""),
            row("py:pkg/money.py::format_money", "format_money", ""),
            row("py:pkg/money.py::format_money", "format_money", ""),
        ]
    );
}

#[test]
fn a_target_in_a_skipped_file_gives_the_reason_the_scan_skipped_it() {
    let big = format!("{}def x():\n    return 1\n", "y = 1\n".repeat(200_000));
    let files = [("a.py", "def helper():\n    return 1\n"), ("big.py", &big)];
    let skipped = format!(
        "the scan did not read big.py (skipped: {} bytes exceeds the per-file cap of 1000000 \
         bytes)",
        big.len()
    );
    assert_eq!(
        bound(&files, &["big.py::x", "big.py:200001"], 10),
        [
            status(
                "not_found",
                &format!("no symbol matches big.py::x: {skipped}")
            ),
            status(
                "not_found",
                &format!("no symbol matches big.py:200001: {skipped}")
            ),
        ]
    );
}

#[test]
fn a_line_of_a_data_file_names_the_data_file_and_the_key_that_spans_the_line() {
    let files = [
        ("conf.json", "{\n  \"name\": \"x\",\n  \"version\": 1\n}\n"),
        ("a.py", "def f():\n    return 1\n"),
    ];
    let data = "conf.json is a data file, and its keys are not symbols that refs, explain or path \
                bind";
    assert_eq!(
        bound(&files, &["conf.json:3", "conf.json:1"], 10),
        [
            status(
                "not_found",
                &format!(
                    "no symbol matches conf.json:3: {data}. The key json:conf.json::version spans \
                     line 3. Expand that id to read the key."
                )
            ),
            status(
                "not_found",
                &format!("no symbol matches conf.json:1: {data}")
            ),
        ]
    );
}

#[test]
fn a_target_that_names_a_file_lists_the_definitions_in_that_file_first() {
    let files = [
        ("a.py", "def run():\n    return 1\n"),
        ("b.py", "def run():\n    return 2\n"),
    ];
    let advice = "2 definitions of run, and 1 is listed: py:b.py::run. Pass one as the target.";
    assert_eq!(
        bound(&files, &["b.py::run#2", "py:b.py::run#2"], 1),
        [
            status(
                "not_found",
                &format!("no symbol matches b.py::run#2: b.py does not define run#2. {advice}")
            ),
            status(
                "not_found",
                &format!("no symbol matches py:b.py::run#2: b.py does not define run#2. {advice}")
            ),
        ]
    );
}

#[test]
fn a_path_with_a_rust_style_method_binds_its_one_definition() {
    let files = [(
        "jobs.py",
        "class Job:\n    def run(self):\n        return 2\n",
    )];
    assert_eq!(
        bound(&files, &["jobs.py::Job::run"], 10),
        [status("bound", "")]
    );
}

#[test]
fn a_target_below_the_root_binds_and_an_absolute_path_outside_it_is_refused() {
    let texts = [
        "./pkg/money.py::format_money",
        "<ROOT>/pkg/money.py:2",
        "/elsewhere/x.py::f",
    ];
    let outside = "no symbol matches /elsewhere/x.py::f: path refused: resolved to \
                   /elsewhere/x.py, which is outside the root <ROOT>";
    assert_eq!(
        bound(&LINKED, &texts, 10),
        [
            status("bound", ""),
            status("bound", ""),
            status("not_found", outside),
        ]
    );
}
