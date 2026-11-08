use std::path::Path;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::{FilesFn, LedgerMark, answer, in_process};
use crate::testing::{Rig, Steps};

fn trace_payload(session: &str, agent: Option<&str>, target: &Value, cwd: &Path) -> String {
    let mut payload = json!({
        "session_id": session,
        "cwd": cwd,
        "hook_event_name": "PostToolUse",
        "tool_name": "mcp__agentless__trace",
        "tool_input": {"target": target},
        "tool_response": [{"type": "text", "text": "not read"}],
    });
    if let Some(agent) = agent {
        payload["agent_id"] = json!(agent);
    }
    payload.to_string()
}

impl Rig {
    fn mark_with(&self, files: FilesFn, deadline: Duration, stdin: &str) -> String {
        let clock = Steps::fixed();
        let hook = LedgerMark {
            run: self.run(&clock),
            files,
            deadline,
        };
        let mut stdout = Vec::new();
        answer(&hook, &self.services, &mut stdin.as_bytes(), &mut stdout);
        String::from_utf8(stdout).expect("UTF-8 stdout")
    }

    fn mark(&self, stdin: &str) {
        let stdout = self.mark_with(in_process, Duration::from_secs(30), stdin);
        assert_eq!(stdout, "");
    }
}

#[test]
fn a_traced_symbol_and_a_traced_file_get_no_card_on_a_later_read() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    let targets = json!(["InvoiceTotal", Rig::REPORT, "InvoiceTotal"]);
    rig.mark(&trace_payload("s1", None, &targets, &root));
    assert_eq!(rig.read_reply("s1", None, Rig::INVOICE), "");
    assert_eq!(rig.read_reply("s1", None, Rig::REPORT), "");
    assert!(!rig.read_reply("s1", Some("a1"), Rig::INVOICE).is_empty());
    assert_eq!(rig.reasons(), ["marked", "held", "held", "emitted"]);
}

#[test]
fn a_subagent_trace_marks_only_its_own_context() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    rig.mark(&trace_payload(
        "s1",
        Some("a1"),
        &json!("round_cents"),
        &root,
    ));
    assert_eq!(rig.read_reply("s1", Some("a1"), Rig::INVOICE), "");
    assert!(!rig.read_reply("s1", None, Rig::INVOICE).is_empty());
    assert_eq!(rig.reasons(), ["marked", "held", "emitted"]);
}

#[test]
fn a_target_with_no_symbol_or_file_card_marks_nothing() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    for (session, target) in [
        ("s1", json!("NothingMatchesThis")),
        ("s2", json!("src/billing")),
        ("s3", json!("round")),
    ] {
        rig.mark(&trace_payload(session, None, &target, &root));
    }
    rig.mark(&trace_payload("s4", None, &json!("round_cents"), &root));
    rig.mark(&trace_payload("s4", None, &json!("round_cents"), &root));
    let expected = [
        "no_card_file",
        "no_card_file",
        "no_card_file",
        "marked",
        "held",
    ];
    assert_eq!(rig.reasons(), expected);
}

#[test]
fn the_call_repo_root_names_the_repository() {
    let rig = Rig::new();
    let elsewhere = rig.sandbox.mkdir("elsewhere");
    let mut payload: Value =
        serde_json::from_str(&trace_payload("s1", None, &json!(Rig::INVOICE), &elsewhere))
            .expect("a payload");
    rig.mark(&payload.to_string());
    payload["tool_input"]["repo_root"] = json!(rig.sandbox.root());
    rig.mark(&payload.to_string());
    assert_eq!(rig.read_reply("s1", None, Rig::INVOICE), "");
    assert_eq!(rig.reasons(), ["no_repository", "marked", "held"]);
}

const STUCK: FilesFn = |_, _, _| {
    loop {
        thread::park();
    }
};

const FAILING: FilesFn =
    |_, _, _| Err(kernel::Error::OperationFailed("no cache root".to_owned()).into());

#[test]
fn a_malformed_late_or_failed_mark_records_nothing() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    let traced = trace_payload("s1", None, &json!("InvoiceTotal"), &root);
    for stdin in [
        "{not json".to_owned(),
        json!({"session_id": "s1", "tool_input": {"target": 7}}).to_string(),
        json!({"session_id": "s1", "tool_input": {"target": []}}).to_string(),
        json!({"session_id": "s1", "tool_input": {"target": ["a", 7]}}).to_string(),
        json!({"session_id": "s1"}).to_string(),
    ] {
        rig.mark(&stdin);
    }
    let late = rig.mark_with(STUCK, Duration::from_millis(100), &traced);
    let failed = rig.mark_with(FAILING, Duration::from_secs(5), &traced);
    assert_eq!((late.as_str(), failed.as_str()), ("", ""));
    assert!(!rig.read_reply("s1", None, Rig::INVOICE).is_empty());
    let expected = [
        "malformed_payload",
        "malformed_payload",
        "malformed_payload",
        "malformed_payload",
        "malformed_payload",
        "deadline",
        "resolve_failed",
        "emitted",
    ];
    assert_eq!(rig.reasons(), expected);
}
