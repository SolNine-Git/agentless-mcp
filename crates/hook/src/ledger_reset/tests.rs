use serde_json::json;

use super::answer;
use crate::testing::{Rig, Steps};

impl Rig {
    fn reset(&self, stdin: &str) {
        let clock = Steps::fixed();
        answer(&self.run(&clock), &mut stdin.as_bytes());
    }
}

fn compact(session: &str) -> String {
    json!({
        "session_id": session,
        "hook_event_name": "SessionStart",
        "source": "compact",
    })
    .to_string()
}

#[test]
fn a_compaction_resets_the_main_context_and_keeps_each_subagent() {
    let rig = Rig::new();
    assert!(!rig.read_reply("s1", None, Rig::INVOICE).is_empty());
    assert!(!rig.read_reply("s1", Some("a1"), Rig::INVOICE).is_empty());
    assert!(!rig.read_reply("s2", None, Rig::INVOICE).is_empty());
    rig.reset(&compact("s1"));
    assert!(!rig.read_reply("s1", None, Rig::INVOICE).is_empty());
    assert_eq!(rig.read_reply("s1", Some("a1"), Rig::INVOICE), "");
    assert_eq!(rig.read_reply("s2", None, Rig::INVOICE), "");
    let expected = [
        "emitted", "emitted", "emitted", "reset", "emitted", "held", "held",
    ];
    assert_eq!(rig.reasons(), expected);
}

#[test]
fn a_reset_with_nothing_to_remove_or_no_session_is_silent() {
    let rig = Rig::new();
    rig.reset(&compact("s1"));
    rig.reset("{not json");
    rig.reset(&json!({"source": "compact"}).to_string());
    rig.sandbox.mkdir("ledger");
    rig.sandbox.set_mode("ledger", 0o770);
    rig.reset(&compact("s1"));
    let expected = [
        "nothing_to_reset",
        "malformed_payload",
        "no_session_id",
        "ledger_storage_unavailable",
    ];
    assert_eq!(rig.reasons(), expected);
    let line = &rig.lines()[0];
    assert_eq!(
        (&line["hook"], &line["event"]),
        (&json!("ledger-reset"), &json!("silent"))
    );
}
