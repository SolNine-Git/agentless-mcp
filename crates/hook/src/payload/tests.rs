use std::path::Path;

use io::testing::cases;
use serde_json::{Value, json};
use text::CodePointString;

use super::{Change, Payload, document, identity, payload, read};
use crate::halt::Halt;
use crate::record::Identity;
use crate::testing::{MANIFEST_DIR, latin};

// The raw payload: a string with one character per byte, or [text, count] for a repeated text.
fn raw(value: &Value) -> Vec<u8> {
    match value {
        Value::String(text) => latin(text),
        Value::Array(parts) => {
            let text = parts[0].as_str().expect("the repeated text");
            let count = parts[1].as_u64().expect("the repeat count");
            latin(&text.repeat(usize::try_from(count).expect("a count that fits")))
        }
        other => panic!("no raw payload form {other}"),
    }
}

// The gate's first steps, as far as the payload goes.
fn parsed(bytes: &[u8]) -> (Identity, Result<Payload, Halt>) {
    let mut stdin = bytes;
    let document = match read(&mut stdin).and_then(|raw| document(&raw)) {
        Ok(document) => document,
        Err(halt) => return (Identity::default(), Err(halt)),
    };
    (identity(&document), payload(&document))
}

fn text(value: &CodePointString) -> Value {
    value.as_str().map_or(Value::Null, |text| json!(text))
}

// The payload without its path, which the caller compares as a path.
fn shown(result: Result<Payload, Halt>) -> (Option<String>, Value) {
    match result {
        Ok(Payload { file_path, change }) => {
            let path = file_path.to_str().expect("a UTF-8 path").to_owned();
            let change = match change {
                Change::Write(content) => json!(["write", text(&content)]),
                Change::Edit(edit) => json!([
                    "edit",
                    text(&edit.old_string),
                    text(&edit.new_string),
                    edit.replace_all
                ]),
            };
            (Some(path), json!({ "change": change }))
        }
        Err(halt) => (None, json!({"halt": [halt.outcome, halt.reason]})),
    }
}

#[test]
fn each_payload_reads_as_the_generated_case_says() {
    let all: Vec<(String, Value, Value)> = cases(MANIFEST_DIR, "payload", "payload");
    assert!(!all.is_empty());
    for (name, given, expected) in all {
        let (identity, result) = parsed(&raw(&given));
        let ids = json!([
            identity.session_id,
            identity.transcript_path,
            identity.tool_use_id,
            identity.tool_name
        ]);
        assert_eq!(ids, expected["identity"], "case {name}");
        let (path, found) = shown(result);
        // CPython prints the joined path in its normal form; Path equality compares components.
        let expected_path = expected["path"].as_str().map(Path::new);
        assert_eq!(path.as_deref().map(Path::new), expected_path, "case {name}");
        assert_eq!(found, expected["result"], "case {name}");
    }
}

#[test]
fn a_file_path_with_a_lone_surrogate_is_malformed() {
    let raw =
        br#"{"tool_name": "Write", "tool_input": {"file_path": "/a/\ud800.py", "content": ""},
        "session_id": "s", "transcript_path": "t", "tool_use_id": "u"}"#;
    let (_, result) = parsed(raw);
    let halt = result.expect_err("the path has no UTF-8 form");
    assert_eq!(
        halt.reason,
        "malformed payload: tool_input.file_path holds a lone surrogate"
    );
}
