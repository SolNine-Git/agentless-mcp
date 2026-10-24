use serde_json::{Value as Json, json};

use crate::Server;
use crate::tests::{Sandbox, ToolAnswer};

// A committed repository with one five-line file, and the server of that repository.
fn served() -> (Sandbox, Server) {
    let sandbox = Sandbox::new();
    sandbox.step(&["init", "repo"]);
    sandbox.step(&[
        "write",
        "repo/five.py",
        "a = 1\nb = 2\nc = 3\nd = 4\ne = 5\n",
    ]);
    sandbox.step(&["commit", "repo"]);
    let server = sandbox.server(&["repo"], &[], Some("repo"));
    (sandbox, server)
}

fn sliced(server: &Server, lines: &Json) -> ToolAnswer {
    let arguments =
        json!({"operation": "slice", "path": "five.py", "lines": lines, "context_lines": 0});
    server.answer("read", arguments.as_object().cloned())
}

#[test]
fn a_flat_pair_answers_as_the_list_of_that_one_pair() {
    let (_sandbox, server) = served();
    let nested = sliced(&server, &json!([[3, 5]]));
    assert!(!nested.is_error, "{}", nested.text);
    assert!(
        nested.text.contains("3| c = 3\n4| d = 4\n5| e = 5\n"),
        "{}",
        nested.text
    );
    assert_eq!(sliced(&server, &json!([3, 5])), nested);
}

// The validation refuses every other flat list, so no slice runs.
#[test]
fn every_other_flat_shape_is_refused() {
    let (_sandbox, server) = served();
    for lines in [
        json!([5]),
        json!([1, 2, 3]),
        json!([1, 2, 3, 4]),
        json!([[1, 2], 3]),
    ] {
        let answer = sliced(&server, &lines);
        assert!(answer.is_error, "{lines}: {}", answer.text);
        assert!(
            answer.text.contains(" for call[read]\nlines."),
            "{lines}: {}",
            answer.text
        );
    }
}
