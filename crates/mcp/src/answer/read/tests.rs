use prompts::envelope::RECEIPT_HEADER;
use serde_json::{Value as Json, json};

use crate::Server;
use crate::tests::{Sandbox, ToolAnswer};

fn refused(text: &str) -> ToolAnswer {
    let text = format!("Error calling tool 'read': {text}");
    ToolAnswer {
        text,
        is_error: true,
    }
}

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
        let want =
            format!("Error calling tool 'read': lines is {lines}, but must be one [start, end]");
        assert!(answer.text.starts_with(&want), "{lines}: {}", answer.text);
    }
}

#[test]
fn a_slice_of_a_path_that_does_not_read_is_an_error_with_the_reason() {
    let (_sandbox, server) = served();
    let missing = "no/such.py: unreadable: No such file or directory";
    let cases = [
        (json!({"path": "no/such.py", "lines": [1, 5]}), missing),
        (json!({"path": "no/such.py", "whole_file": true}), missing),
        (
            json!({"path": ".", "whole_file": true}),
            ".: unreadable: Is a directory",
        ),
    ];
    for (mut arguments, reason) in cases {
        arguments["operation"] = json!("slice");
        let answer = server.answer("read", arguments.as_object().cloned());
        assert_eq!(answer, refused(reason), "{arguments}");
    }
}

#[test]
fn a_slice_whose_every_range_starts_past_the_end_is_an_error_with_each_range() {
    let (_sandbox, server) = served();
    let reasons = "unsatisfiable: line range 9-12 is beyond five.py (5 lines)\n\
        unsatisfiable: line range 40-41 is beyond five.py (5 lines)";
    assert_eq!(
        sliced(&server, &json!([[9, 12], [40, 41]])),
        refused(reasons)
    );
}

#[test]
fn a_slice_with_one_range_inside_the_file_answers_and_names_the_other_range() {
    let (_sandbox, server) = served();
    let answer = sliced(&server, &json!([[2, 3], [9, 12]]));
    assert!(!answer.is_error, "{}", answer.text);
    let (receipt, body) = answer.text.split_once('\n').expect("a receipt line");
    assert!(receipt.starts_with(RECEIPT_HEADER), "{receipt}");
    let expected = "unsatisfiable: line range 9-12 is beyond five.py (5 lines)\n\
        ...\n2| b = 2\n3| c = 3\n...\n";
    assert_eq!(body, expected);
}
