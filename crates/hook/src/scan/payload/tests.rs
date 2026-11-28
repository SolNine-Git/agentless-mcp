use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Action, Payload, parse};
use crate::halt::Halt;

fn parsed(value: &Value) -> Result<Payload, Halt> {
    parse(value.to_string().as_bytes())
}

fn reason(value: &Value) -> String {
    parsed(value).expect_err("the payload is refused").reason
}

#[test]
fn a_post_tool_use_payload_gives_the_session_the_folder_and_the_command() {
    let payload = parsed(&json!({
        "hook_event_name": "PostToolUse",
        "session_id": "s1",
        "cwd": "/work/repo",
        "tool_input": {"command": "cat > a.py"},
    }))
    .expect("a payload");

    let expected = Payload {
        event: "PostToolUse",
        session: "s1".to_owned(),
        cwd: PathBuf::from("/work/repo"),
        action: Action::Shell("cat > a.py".to_owned()),
    };
    assert_eq!(payload, expected);
}

#[test]
fn a_stop_and_a_subagent_stop_payload_give_the_stop_flag() {
    for (name, active) in [("Stop", false), ("SubagentStop", true)] {
        let payload = parsed(&json!({
            "hook_event_name": name,
            "session_id": "s1",
            "cwd": "/work/repo",
            "stop_hook_active": active,
        }))
        .expect("a payload");

        assert_eq!(payload.event, name);
        assert_eq!(payload.action, Action::Stop { active });
    }
}

#[test]
fn a_missing_or_mistyped_field_fails_the_parse() {
    let post = json!({
        "hook_event_name": "PostToolUse",
        "session_id": "s1",
        "cwd": "/work/repo",
        "tool_input": {"command": "cat > a.py"},
    });
    let stop = json!({
        "hook_event_name": "Stop",
        "session_id": "s1",
        "cwd": "/work/repo",
        "stop_hook_active": false,
    });
    let cases = [
        (
            &post,
            "session_id",
            "malformed payload: payload.session_id is missing",
        ),
        (&post, "cwd", "malformed payload: payload.cwd is missing"),
        (
            &post,
            "tool_input",
            "malformed payload: payload.tool_input is missing",
        ),
        (
            &stop,
            "stop_hook_active",
            "malformed payload: payload.stop_hook_active is missing",
        ),
        (
            &stop,
            "hook_event_name",
            "malformed payload: payload.hook_event_name is missing",
        ),
    ];
    for (base, field, expected) in cases {
        let mut broken = base.clone();
        broken.as_object_mut().expect("an object").remove(field);
        assert_eq!(reason(&broken), expected, "{field}");
    }
    let mut mistyped = stop.clone();
    mistyped["stop_hook_active"] = json!("no");
    assert_eq!(
        reason(&mistyped),
        "malformed payload: payload.stop_hook_active is not a bool"
    );
    let mut no_command = post.clone();
    no_command["tool_input"] = json!({});
    assert_eq!(
        reason(&no_command),
        "malformed payload: tool_input.command is missing"
    );
}

#[test]
fn the_erosion_tool_use_id_is_not_required() {
    let post = json!({
        "hook_event_name": "PostToolUse",
        "session_id": "s1",
        "cwd": "/work/repo",
        "tool_input": {"command": "ls"},
    });
    assert!(post.get("tool_use_id").is_none());
    assert!(parsed(&post).is_ok());
}

#[test]
fn another_event_and_a_payload_that_is_no_object_are_refused() {
    let other = json!({"hook_event_name": "PreToolUse", "session_id": "s1", "cwd": "/w"});
    assert_eq!(reason(&other), "not a scanned event: PreToolUse");
    assert_eq!(
        parse(b"[1]").expect_err("refused").reason,
        "malformed payload: the payload is not a JSON object"
    );
    assert!(parse(b"{").is_err());
}

#[test]
fn a_lone_surrogate_in_a_text_field_fails_the_parse() {
    let raw = br#"{"hook_event_name": "Stop", "session_id": "\ud800", "cwd": "/w", "stop_hook_active": false}"#;
    assert_eq!(
        parse(raw).expect_err("refused").reason,
        "malformed payload: payload.session_id holds a lone surrogate"
    );
}
