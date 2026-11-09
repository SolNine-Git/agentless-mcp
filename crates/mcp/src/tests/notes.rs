use serde_json::json;

use super::{Setup, probe, quiet_ports};
use crate::HookCheck;

const NOTE: &str = "// note: hooks: 7 of 7 agentless hook entries are missing (read-card, \
     ledger-mark, brief-record, subagent-map, prompt-map, erosion, ledger-reset). Run: \
     agentless-mcp hook install";

// The first answer of each client session carries the note under the receipt line; a refusal has
// no receipt, so it leaves the note for the next answer.
#[test]
fn the_first_answer_of_each_session_carries_the_hook_note() {
    let sandbox = probe();
    sandbox.step(&["write", "config/settings.json", "{\"hooks\": {}}"]);
    let check = HookCheck {
        user_settings: Some(sandbox.root.join("config/settings.json")),
        binary: sandbox.root.join("bin/agentless-mcp"),
    };
    let setup = Setup {
        roots: &["repo"],
        files: &[],
        launch: None,
    };
    let server = sandbox.server_checking(&setup, sandbox.services(), quiet_ports(), Some(check));
    let trace = json!({"target": "alpha"});
    let second_line = |server: &crate::Server, arguments: &serde_json::Value| {
        let answer = server.answer("trace", arguments.as_object().cloned());
        assert!(!answer.is_error, "{}", answer.text);
        answer.text.lines().nth(1).map(str::to_owned)
    };
    assert!(server.answer("trace", None).is_error);
    assert_eq!(second_line(&server, &trace).as_deref(), Some(NOTE));
    let card = second_line(&server, &trace);
    assert!(card.is_some_and(|line| !line.starts_with("// ")));
    let other = server.session();
    assert_eq!(second_line(&other, &trace).as_deref(), Some(NOTE));
    sandbox.step(&["write", "repo/untracked.py", "x = 1\n"]);
    assert_eq!(
        second_line(&server.session(), &trace).as_deref(),
        Some(NOTE)
    );
}

// The note follows the notes of the git state on their one line.
#[test]
fn the_hook_note_follows_the_git_notes() {
    let sandbox = super::Sandbox::new();
    sandbox.step(&["write", "plain/a.py", "def a():\n    return 1\n"]);
    let check = HookCheck {
        user_settings: None,
        binary: sandbox.root.join("bin/agentless-mcp"),
    };
    let setup = Setup {
        roots: &["plain"],
        files: &[],
        launch: None,
    };
    let server = sandbox.server_checking(&setup, sandbox.services(), quiet_ports(), Some(check));
    let answer = server.answer("search", json!({"files": "*"}).as_object().cloned());
    let note = sandbox.normalised(answer.text.lines().nth(1).unwrap_or_default());
    let git = "// note: <SANDBOX>/plain is not inside a git repository: HEAD and dirty count are \
               unknown; hooks: ";
    assert!(note.starts_with(git), "{}", answer.text);
    assert_eq!(
        answer.text.matches("// note: ").count(),
        1,
        "{}",
        answer.text
    );
}
