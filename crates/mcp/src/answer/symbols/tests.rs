use prompts::envelope::RECEIPT_HEADER;
use serde_json::{Value as Json, json};

use crate::Server;
use crate::tests::{Sandbox, ToolAnswer};

const UNREADABLE: &str = "no/such.py: unreadable: No such file or directory";

// A committed repository with one function and one file that no grammar reads, a file beside it,
// and the server of that repository.
fn served() -> (Sandbox, Server) {
    let sandbox = Sandbox::new();
    sandbox.step(&["write", "outside.py", "x = 1\n"]);
    sandbox.step(&["init", "repo"]);
    sandbox.step(&["write", "repo/shapes.py", "def area():\n    return 0\n"]);
    sandbox.step(&["write", "repo/README.md", "# shapes\n"]);
    sandbox.step(&["commit", "repo"]);
    let server = sandbox.server(&["repo"], &[], Some("repo"));
    (sandbox, server)
}

fn called(server: &Server, arguments: &Json) -> ToolAnswer {
    server.answer("symbols", arguments.as_object().cloned())
}

fn refused(text: &str) -> ToolAnswer {
    let text = format!("Error calling tool 'symbols': {text}");
    ToolAnswer {
        text,
        is_error: true,
    }
}

// The text under the receipt line of an answer that is no error.
fn body(answer: &ToolAnswer) -> &str {
    assert!(!answer.is_error, "{}", answer.text);
    let (receipt, body) = answer.text.split_once('\n').expect("a receipt line");
    assert!(receipt.starts_with(RECEIPT_HEADER), "{receipt}");
    body
}

#[test]
fn an_overview_whose_every_path_fails_is_an_error_with_each_reason() {
    let (sandbox, server) = served();
    let missing = json!({"operation": "overview", "paths": ["no/such.py"]});
    assert_eq!(called(&server, &missing), refused(UNREADABLE));
    let paths = ["no/such.py", "../outside.py", "README.md"];
    let mixed = json!({"operation": "overview", "paths": paths});
    let outside = sandbox.placed(
        "path refused: resolved to <SANDBOX>/outside.py, which is outside the root <SANDBOX>/repo",
    );
    let reasons = format!("{UNREADABLE}\n{outside}\nREADME.md: no grammar for this file type");
    assert_eq!(called(&server, &mixed), refused(&reasons));
}

#[test]
fn an_overview_with_one_readable_path_answers_and_names_the_other_path() {
    let (_sandbox, server) = served();
    let arguments = json!({"operation": "overview", "paths": ["no/such.py", "shapes.py"]});
    let expected = format!(
        "no/such.py  (python)\n  {UNREADABLE}\nshapes.py  (python)\n  stable ids: \
         py:shapes.py::<QualifiedName> -- nested symbols qualify as Class.method\n\
         def area():\n    ...\n"
    );
    assert_eq!(body(&called(&server, &arguments)), expected);
}

#[test]
fn an_expansion_where_no_id_resolves_is_an_error_with_each_row() {
    let (_sandbox, server) = served();
    let ids = ["py:shapes.py::gone", "py:no/such.py::f"];
    let arguments = json!({"operation": "expand", "stable_ids": ids});
    let rows = format!(
        "unresolved: py:shapes.py::gone -- shapes.py does not define gone\n\
         unresolved: py:no/such.py::f -- {UNREADABLE}"
    );
    assert_eq!(called(&server, &arguments), refused(&rows));
}

#[test]
fn an_expansion_with_one_resolved_id_answers_and_names_the_other_id() {
    let (_sandbox, server) = served();
    let ids = ["py:shapes.py::area", "py:shapes.py::gone"];
    let arguments = json!({"operation": "expand", "stable_ids": ids});
    let expected = "[py:shapes.py::area] @1-2\n  function\n  1| def area():\n  2|     return 0\n\n\
        unresolved: py:shapes.py::gone -- shapes.py does not define gone\n";
    assert_eq!(body(&called(&server, &arguments)), expected);
}
