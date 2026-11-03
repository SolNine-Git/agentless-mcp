use serde_json::{Value as Json, json};

use crate::Server;
use crate::tests::{Sandbox, ToolAnswer};

// A committed repository with one helper and one code file over the per-file cap, its size, and
// the server of that repository.
fn served_with_a_skipped_file() -> (Sandbox, Server, usize) {
    let big = format!("{}def x():\n    return 1\n", "y = 1\n".repeat(200_000));
    let sandbox = Sandbox::new();
    sandbox.step(&["init", "repo"]);
    sandbox.step(&["write", "repo/a.py", "def helper():\n    return 1\n"]);
    sandbox.step(&["write", "repo/big.py", &big]);
    sandbox.step(&["commit", "repo"]);
    let server = sandbox.server(&["repo"], &[], Some("repo"));
    (sandbox, server, big.len())
}

fn called(server: &Server, tool: &str, arguments: &Json) -> ToolAnswer {
    server.answer(tool, arguments.as_object().cloned())
}

fn refused(tool: &str, text: &str) -> ToolAnswer {
    ToolAnswer {
        text: format!("Error calling tool '{tool}': {text}"),
        is_error: true,
    }
}

#[test]
fn a_refusal_of_a_target_that_binds_to_nothing_names_the_skipped_code_files() {
    let (_sandbox, server, size) = served_with_a_skipped_file();
    let reason = format!("skipped: {size} bytes exceeds the per-file cap of 1000000 bytes");
    let warning = format!(
        "// warning: 1 file was skipped and is absent from this answer: big.py ({reason}). A \
         symbol in a skipped file reads as missing here even though it may exist in the \
         repository."
    );
    let in_skipped = format!(
        "no symbol matches py:big.py::x: the scan did not read big.py ({reason})\n{warning}"
    );
    let refs = |target: &str| {
        called(
            &server,
            "find_referencing_symbols",
            &json!({"target": target}),
        )
    };
    assert_eq!(
        refs("py:big.py::x"),
        refused("find_referencing_symbols", &in_skipped)
    );
    assert_eq!(
        refs("zz_none"),
        refused(
            "find_referencing_symbols",
            &format!("no symbol matches zz_none\n{warning}")
        )
    );
    let explain = json!({"operation": "explain", "target": "py:big.py::x"});
    assert_eq!(
        called(&server, "symbols", &explain),
        refused("symbols", &in_skipped)
    );
}
