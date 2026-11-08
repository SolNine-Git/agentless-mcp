use std::ffi::OsString;
use std::path::PathBuf;

use super::{
    DEFAULT_SERVER_NAME, EntryState, Event, Expected, Found, Handler, Hook, HookStatus, HooksShape,
    Named, Retired, command, compare, entries, handlers, named, settings_handlers, user_settings,
};
use crate::EnvConfig;

const OWN: &str = "/home/user/.local/bin/agentless-mcp";

// The settings file of the 0.11.0 hooks, contrib/hooks/settings-example.json with a real path.
const OLD_RELEASE: &str = r#"{
  "PreToolUse": [
    {"matcher": "Grep|Glob|Bash", "hooks": [
      {"type": "command", "command": "/home/user/.local/bin/agentless-mcp hook gate-check"}]},
    {"matcher": "Write|Edit", "hooks": [
      {"type": "command", "command": "/home/user/.local/bin/agentless-mcp hook erosion", "timeout": 15}]}
  ],
  "PostToolUse": [
    {"matcher": "mcp__agentless__.*", "hooks": [
      {"type": "command", "command": "/home/user/.local/bin/agentless-mcp hook gate-mark"}]}
  ],
  "UserPromptSubmit": [
    {"hooks": [
      {"type": "command", "command": "/home/user/.local/bin/agentless-mcp hook prompt-map", "timeout": 15}]}
  ]
}"#;

fn status(settings: &str, server_name: &str) -> HookStatus {
    let value = json::parse(settings).unwrap_or_else(|error| panic!("{error}"));
    let read = handlers(&value).unwrap_or_else(|shape| panic!("{shape:?}"));
    compare(&read, OWN, server_name)
}

fn state_of(status: &HookStatus, hook: Hook) -> &EntryState {
    let row = status.rows.iter().find(|row| row.entry.hook == hook);
    &row.unwrap_or_else(|| panic!("{hook:?} has a row")).state
}

// The settings that `hook install` would write from the table, with a foreign handler in one
// group and a foreign group, as a hand-edited file holds them.
fn installed(server_name: &str) -> String {
    let mut events: Vec<(Event, Vec<String>)> = Vec::new();
    for entry in entries(server_name) {
        let matcher =
            (entry.matcher.as_ref()).map_or(String::new(), |m| format!("\"matcher\": \"{m}\", "));
        let mut handlers = vec![format!(
            "{{\"type\": \"command\", \"command\": \"{}\", \"timeout\": {}}}",
            command(OWN, entry.hook),
            entry.timeout
        )];
        if entry.hook == Hook::Erosion {
            handlers.push("{\"type\": \"command\", \"command\": \"prettier --check\"}".to_owned());
        }
        let group = format!("{{{matcher}\"hooks\": [{}]}}", handlers.join(", "));
        match events.iter_mut().find(|(event, _)| *event == entry.event) {
            Some((_, groups)) => groups.push(group),
            None => events.push((entry.event, vec![group])),
        }
    }
    events[0].1.push(
        "{\"matcher\": \"Bash\", \"hooks\": [{\"type\": \"prompt\", \"prompt\": \"x\"}]}"
            .to_owned(),
    );
    let body: Vec<String> = (events.iter())
        .map(|(event, groups)| format!("\"{}\": [{}]", event.as_str(), groups.join(", ")))
        .collect();
    format!("{{{}}}", body.join(", "))
}

#[test]
fn the_table_holds_each_hook_with_its_event_matcher_and_three_times_its_deadline() {
    let rows: Vec<(&str, &str, Option<String>, u32)> = (entries("agentless").into_iter())
        .map(|entry| {
            (
                entry.hook.name(),
                entry.event.as_str(),
                entry.matcher,
                entry.timeout,
            )
        })
        .collect();
    let some = |matcher: &str| Some(matcher.to_owned());
    assert_eq!(
        rows,
        [
            ("read-card", "PostToolUse", some("Read|Bash"), 9),
            (
                "ledger-mark",
                "PostToolUse",
                some("mcp__agentless__trace"),
                6
            ),
            ("brief-record", "PreToolUse", some("Agent"), 3),
            ("subagent-map", "SubagentStart", None, 15),
            ("prompt-map", "UserPromptSubmit", None, 15),
            ("erosion", "PreToolUse", some("Write|Edit"), 15),
            ("ledger-reset", "SessionStart", some("compact"), 3),
        ]
    );
    let other = entries("tools");
    assert_eq!(other[1].matcher.as_deref(), Some("mcp__tools__trace"));
}

#[test]
fn an_empty_hooks_value_has_every_entry_missing() {
    let empty = status("{}", DEFAULT_SERVER_NAME);
    assert!(
        empty
            .rows
            .iter()
            .all(|row| row.state == EntryState::Missing)
    );
    assert!(empty.retired.is_empty() && !empty.is_current());
}

#[test]
fn the_old_release_has_two_retired_handlers_two_current_entries_and_the_rest_missing() {
    let old = status(OLD_RELEASE, DEFAULT_SERVER_NAME);
    let retired: Vec<Retired> = old.retired.iter().map(|entry| entry.hook).collect();
    assert_eq!(retired, [Retired::GateCheck, Retired::GateMark]);
    assert_eq!(state_of(&old, Hook::Erosion), &EntryState::Current);
    assert_eq!(state_of(&old, Hook::PromptMap), &EntryState::Current);
    for hook in [
        Hook::ReadCard,
        Hook::LedgerMark,
        Hook::BriefRecord,
        Hook::SubagentMap,
    ] {
        assert_eq!(state_of(&old, hook), &EntryState::Missing, "{hook:?}");
    }
    assert!(!old.is_current());
}

#[test]
fn an_installed_table_with_foreign_handlers_is_current() {
    let current = status(&installed("agentless"), "agentless");
    assert!(current.is_current(), "{current:?}");
}

#[test]
fn another_binary_or_another_server_name_is_not_current() {
    let moved = installed("agentless").replace(OWN, "/opt/old/agentless-mcp");
    let elsewhere = status(&moved, "agentless");
    let expected = EntryState::OtherPath("/opt/old/agentless-mcp".to_owned());
    assert_eq!(state_of(&elsewhere, Hook::ReadCard), &expected);
    let renamed = status(&installed("agentless"), "tools");
    assert!(matches!(
        state_of(&renamed, Hook::LedgerMark),
        EntryState::Stale(_)
    ));
    assert_eq!(state_of(&renamed, Hook::ReadCard), &EntryState::Current);
}

#[test]
fn a_changed_timeout_or_a_second_handler_is_stale_and_a_copy_is_one_handler() {
    let slow = installed("agentless").replace("\"timeout\": 9}", "\"timeout\": 30}");
    let EntryState::Stale(found) = state_of(&status(&slow, "agentless"), Hook::ReadCard).clone()
    else {
        panic!("a stale entry");
    };
    let expected = Found {
        event: "PostToolUse".to_owned(),
        matcher: Some("Read|Bash".to_owned()),
        timeout: Some(30.0),
        path: OWN.to_owned(),
    };
    assert_eq!(found, [expected]);
    let line = format!("{OWN} hook read-card");
    let twice = format!(
        "{{\"PostToolUse\": [{{\"matcher\": \"Read|Bash\", \"hooks\": [{{\"type\": \"command\", \
         \"command\": \"{line}\", \"timeout\": 9}}]}}, {{\"matcher\": \"Read|Bash\", \"hooks\": \
         [{{\"type\": \"command\", \"command\": \"{line}\", \"timeout\": 9}}]}}]}}"
    );
    assert_eq!(
        state_of(&status(&twice, "agentless"), Hook::ReadCard),
        &EntryState::Current
    );
    let both = twice.replacen("\"timeout\": 9", "\"timeout\": 8", 1);
    assert!(matches!(
        state_of(&status(&both, "agentless"), Hook::ReadCard),
        EntryState::Stale(_)
    ));
}

#[test]
fn only_a_command_of_the_agentless_binary_with_a_known_hook_is_an_agentless_entry() {
    let foreign = r#"{"PostToolUse": [{"matcher": "Read|Bash", "hooks": [
        {"type": "command", "command": "/usr/bin/other hook read-card", "timeout": 9},
        {"type": "command", "command": "/x/agentless-mcp hook unknown-hook", "timeout": 9},
        {"type": "command", "command": "/x/agentless-mcp map", "timeout": 9},
        {"type": "prompt", "command": "/x/agentless-mcp hook read-card", "timeout": 9}]}]}"#;
    let read = status(foreign, "agentless");
    assert!(read.rows.iter().all(|row| row.state == EntryState::Missing));
}

#[test]
fn a_path_with_a_space_is_quoted_and_reads_back_as_the_same_path() {
    let spaced = "/home/a user/bin/agentless-mcp";
    let line = command(spaced, Hook::PromptMap);
    assert_eq!(line, "'/home/a user/bin/agentless-mcp' hook prompt-map");
    let settings = format!(
        "{{\"UserPromptSubmit\": [{{\"hooks\": [{{\"type\": \"command\", \"command\": {}, \"timeout\": 15}}]}}]}}",
        serde_json::to_string(&line).unwrap_or_else(|error| panic!("{error}"))
    );
    let value = json::parse(&settings).unwrap_or_else(|error| panic!("{error}"));
    let read = handlers(&value).unwrap_or_else(|shape| panic!("{shape:?}"));
    let read = compare(&read, spaced, "agentless");
    assert_eq!(state_of(&read, Hook::PromptMap), &EntryState::Current);
}

#[test]
fn a_hooks_value_of_another_shape_names_the_first_place_that_differs() {
    let cases = [
        ("[]", "", Expected::Object),
        ("{\"PreToolUse\": {}}", "PreToolUse", Expected::Array),
        ("{\"PreToolUse\": [3]}", "PreToolUse[0]", Expected::Object),
        (
            "{\"PreToolUse\": [{\"matcher\": 1}]}",
            "PreToolUse[0].matcher",
            Expected::Text,
        ),
        (
            "{\"PreToolUse\": [{\"hooks\": {}}]}",
            "PreToolUse[0].hooks",
            Expected::Array,
        ),
        (
            "{\"PreToolUse\": [{\"hooks\": [{\"type\": \"command\", \"timeout\": \"9\"}]}]}",
            "PreToolUse[0].hooks[0].timeout",
            Expected::Number,
        ),
    ];
    for (settings, at, expected) in cases {
        let value = json::parse(settings).unwrap_or_else(|error| panic!("{error}"));
        let refused = handlers(&value);
        let shape = HooksShape {
            at: at.to_owned(),
            expected,
        };
        assert_eq!(refused, Err(shape), "{settings}");
    }
}

// The variables of one environment, by name.
type Vars<'a> = &'a [(&'a str, &'a str)];

#[test]
fn the_user_settings_file_is_under_claude_config_dir_when_it_is_set_and_not_empty() {
    let cases: [(Vars, Option<&str>); 5] = [
        (
            &[("CLAUDE_CONFIG_DIR", "/c"), ("HOME", "/h")],
            Some("/c/settings.json"),
        ),
        (
            &[("CLAUDE_CONFIG_DIR", ""), ("HOME", "/h")],
            Some("/h/.claude/settings.json"),
        ),
        (&[("HOME", "/h")], Some("/h/.claude/settings.json")),
        (&[("CLAUDE_CONFIG_DIR", ""), ("HOME", "")], None),
        (&[], None),
    ];
    for (vars, expected) in cases {
        let vars = (vars.iter()).map(|(name, value)| (OsString::from(name), OsString::from(value)));
        let env = EnvConfig::new(vars, PathBuf::from("/"));
        assert_eq!(user_settings(&env), expected.map(PathBuf::from));
    }
}

#[test]
fn an_agentless_entry_is_a_command_of_the_binary_that_names_a_current_or_retired_hook() {
    let cases = [
        (
            "command",
            "/x/agentless-mcp hook read-card",
            Some(Named::Current(Hook::ReadCard)),
        ),
        (
            "command",
            "'/a b/agentless-mcp' hook gate-mark",
            Some(Named::Retired(Retired::GateMark)),
        ),
        ("command", "/x/agentless-mcp hook nope", None),
        ("command", "/x/agentless-mcp-dev hook read-card", None),
        ("command", "/x/agentless-mcp run read-card", None),
        ("command", "/x/agentless-mcp hook read-card --now", None),
        ("command", "/x/agentless-mcp hook 'read-card", None),
        ("prompt", "/x/agentless-mcp hook read-card", None),
    ];
    for (kind, command, expected) in cases {
        let handler = Handler {
            event: "PostToolUse".to_owned(),
            matcher: None,
            kind: Some(kind.to_owned()),
            command: Some(command.to_owned()),
            timeout: None,
        };
        assert_eq!(named(&handler), expected, "{kind} {command}");
    }
}

#[test]
fn a_settings_document_of_another_shape_names_the_json_path_and_the_type() {
    for (text, expected) in [
        ("[]", "$ is not an object"),
        (r#"{"hooks": []}"#, "$.hooks is not an object"),
        (r#"{"hooks": {"Stop": {}}}"#, "$.hooks.Stop is not an array"),
        (
            r#"{"hooks": {"Stop": [1]}}"#,
            "$.hooks.Stop[0] is not an object",
        ),
        (
            r#"{"hooks": {"Stop": [{"hooks": [{"command": 7}]}]}}"#,
            "$.hooks.Stop[0].hooks[0].command is not a string",
        ),
        (
            r#"{"hooks": {"Stop": [{"hooks": [{"timeout": "3"}]}]}}"#,
            "$.hooks.Stop[0].hooks[0].timeout is not a number",
        ),
    ] {
        let document = json::parse(text).unwrap_or_else(|error| panic!("{error}"));
        let shape = settings_handlers(&document).expect_err(text);
        assert_eq!(shape.text(), expected, "{text}");
    }
    let other = json::parse(r#"{"model": "opus"}"#).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(settings_handlers(&other), Ok(Vec::new()));
}
