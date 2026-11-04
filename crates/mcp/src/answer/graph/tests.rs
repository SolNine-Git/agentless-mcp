use prompts::envelope::RECEIPT_HEADER;
use serde_json::json;

use crate::Server;
use crate::tests::{Sandbox, ToolAnswer};

// A committed repository with two definitions of one name, a data file, and its server.
fn served() -> (Sandbox, Server) {
    let sandbox = Sandbox::new();
    sandbox.step(&["init", "repo"]);
    sandbox.step(&["write", "repo/a.py", "def run():\n    return 1\n"]);
    sandbox.step(&["write", "repo/b.py", "def run():\n    return 2\n"]);
    sandbox.step(&["write", "repo/conf.json", "{\"name\": \"x\"}\n"]);
    sandbox.step(&["commit", "repo"]);
    let server = sandbox.server(&["repo"], &[], Some("repo"));
    (sandbox, server)
}

fn traced(server: &Server, source: &str, target: &str) -> ToolAnswer {
    let arguments = json!({"operation": "path", "source": source, "target": target});
    server.answer("graph", arguments.as_object().cloned())
}

fn refused(text: &str) -> ToolAnswer {
    ToolAnswer {
        text: format!("Error calling tool 'graph': {text}"),
        is_error: true,
    }
}

#[test]
fn a_path_whose_endpoint_matches_nothing_several_symbols_or_a_data_key_is_an_error() {
    let (_sandbox, server) = served();
    assert_eq!(
        traced(&server, "nothing_here", "a.py"),
        refused("no symbol or file matches nothing_here")
    );
    assert_eq!(
        traced(&server, "run", "a.py"),
        refused("run is ambiguous: py:a.py::run, py:b.py::run")
    );
    assert_eq!(
        traced(&server, "json:conf.json::name", "a.py"),
        refused(
            "json:conf.json::name is a data key, and a data key has no edges: Grep finds the \
             spellings of name."
        )
    );
}

#[test]
fn a_source_that_binds_to_nothing_says_to_pass_a_candidate_as_the_source() {
    let (_sandbox, server) = served();
    assert_eq!(
        traced(&server, "py:gone.py::run", "a.py"),
        refused(
            "no symbol or file matches py:gone.py::run: the scan read no file gone.py. 2 \
             definitions of run: py:a.py::run, py:b.py::run. Pass one as the source."
        )
    );
}

#[test]
fn a_path_between_two_resolved_endpoints_with_no_path_is_an_answer_and_no_error() {
    let (_sandbox, server) = served();
    let answer = traced(&server, "py:a.py::run", "py:b.py::run");
    assert!(!answer.is_error, "{}", answer.text);
    let (receipt, body) = answer.text.split_once('\n').expect("a receipt line");
    assert!(receipt.starts_with(RECEIPT_HEADER), "{receipt}");
    assert_eq!(
        body,
        "no path from py:a.py::run to py:b.py::run over resolved edges; unique and \
         name-only-ambiguous edges were excluded\n"
    );
}
