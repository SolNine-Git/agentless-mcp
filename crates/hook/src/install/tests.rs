use std::cell::Cell;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use app::EnvConfig;
use serde_json::{Value, json};

use super::disk::{self, Written};
use super::edit::Removed;
use super::layout::indented;
use super::{installed, statused, uninstall_output, uninstalled};
use crate::Output;
use crate::testing::{MANIFEST_DIR, Sandbox};

const BINARY: &str = "/opt/agentless/bin/agentless-mcp";
const PLACEHOLDER: &str = "/ABSOLUTE/PATH/TO/agentless-mcp";
const FRESH: &str = "added 7, replaced 0, removed 0 retired, kept 0 other handlers";

// A sandbox with a user config folder, a home and a repository, and the environment that names
// them. No test reaches a settings file outside the sandbox.
struct Rig {
    sandbox: Sandbox,
}

impl Rig {
    fn new() -> Self {
        let sandbox = Sandbox::new();
        sandbox.mkdir("config");
        sandbox.mkdir("repo");
        Self { sandbox }
    }

    fn env(&self) -> EnvConfig {
        let config = self.sandbox.path().join("config").into_os_string();
        let env = self.sandbox.env(&[("CLAUDE_CONFIG_DIR", config)]);
        EnvConfig::new(env.vars().to_vec(), self.sandbox.root())
    }

    fn settings(&self) -> PathBuf {
        self.sandbox.path().join("config/settings.json")
    }

    fn put_settings(&self, text: &str) {
        drop(self.sandbox.put("config/settings.json", text));
    }

    fn install(&self, args: &[&str]) -> Output {
        installed(&words(args), &self.env(), BINARY)
    }

    fn status(&self, args: &[&str]) -> Output {
        statused(&words(args), &self.env(), BINARY)
    }

    fn uninstall(&self, args: &[&str]) -> Output {
        uninstalled(&words(args), &self.env())
    }

    fn shown(&self, output: &Output) -> (u8, String, String) {
        let place =
            |text: &str| text.replace(&self.sandbox.path().display().to_string(), "<SANDBOX>");
        (output.code, place(&output.stdout), place(&output.stderr))
    }
}

fn text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn words(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn example() -> String {
    let path = Path::new(MANIFEST_DIR).join("../../contrib/hooks/settings-example.json");
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_settings_example_is_the_dry_run_on_an_empty_file() {
    let rig = Rig::new();
    let env = rig.env();
    let missing = installed(&words(&["--dry-run"]), &env, PLACEHOLDER);
    rig.put_settings("{}");
    let empty = installed(&words(&["--dry-run"]), &env, PLACEHOLDER);

    assert_eq!(missing.stdout, example());
    assert_eq!(empty, missing);
    let report = format!("{FRESH}; --dry-run wrote nothing to <SANDBOX>/config/settings.json\n");
    assert_eq!(rig.shown(&empty), (0, example(), report));
    assert_eq!(text(&rig.settings()), "{}");
}

#[test]
fn an_install_run_twice_is_byte_equal_and_current() {
    let rig = Rig::new();
    let first = rig.install(&[]);
    let written = text(&rig.settings());
    let second = rig.install(&[]);

    let wrote = format!("{FRESH}; wrote <SANDBOX>/config/settings.json\n");
    assert_eq!(rig.shown(&first), (0, wrote, String::new()));
    let unchanged = "no change: <SANDBOX>/config/settings.json holds the current entries\n";
    assert_eq!(rig.shown(&second), (0, unchanged.to_owned(), String::new()));
    assert_eq!(text(&rig.settings()), written);
    assert_eq!(written, example().replace(PLACEHOLDER, BINARY));
    assert_eq!(rig.status(&[]).code, 0);
}

// The 0.11.0 entries, a foreign handler in a group of its own, a group that mixes a foreign
// handler with a retired one, and keys that are not hooks.
const MIXED: &str = r#"{
  "model": "opus",
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Grep|Glob|Bash",
        "hooks": [
          {"type": "command", "command": "/old/bin/agentless-mcp hook gate-check"},
          {"type": "command", "command": "/usr/local/bin/audit-shell"}
        ]
      },
      {
        "matcher": "Write|Edit",
        "hooks": [{"type": "command", "command": "/old/bin/agentless-mcp hook erosion", "timeout": 15}]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "mcp__agentless__.*",
        "hooks": [{"type": "command", "command": "/old/bin/agentless-mcp hook gate-mark"}]
      }
    ],
    "Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}]
  },
  "env": {"NAME": "Z\u00fcrich", "BIG": 12345678901234567890}
}
"#;

#[test]
fn foreign_handlers_mixed_groups_and_other_keys_stay_and_retired_entries_go() {
    let rig = Rig::new();
    rig.put_settings(MIXED);

    let output = rig.install(&[]);

    let report = "added 6, replaced 1, removed 2 retired (gate-check, gate-mark), kept 2 other \
                  handlers; wrote <SANDBOX>/config/settings.json\n";
    assert_eq!(rig.shown(&output), (0, report.to_owned(), String::new()));
    let text = text(&rig.settings());
    let document: Value = serde_json::from_str(&text).expect("JSON");
    // serde_json sorts the keys it reads, so the order is read from the text.
    let at = |key: &str| text.find(&format!("\"{key}\": ")).unwrap_or(usize::MAX);
    let top = ["model", "hooks", "env"].map(at);
    assert!(top.is_sorted(), "{text}");
    let events = [
        "PreToolUse",
        "PostToolUse",
        "Stop",
        "SubagentStart",
        "UserPromptSubmit",
        "SessionStart",
    ];
    assert!(
        events.map(at).is_sorted() && at("SessionStart") < at("env"),
        "{text}"
    );
    let pre = &document["hooks"]["PreToolUse"];
    assert_eq!(
        pre[0],
        json!({"matcher": "Grep|Glob|Bash", "hooks": [{"type": "command", "command": "/usr/local/bin/audit-shell"}]})
    );
    let matchers: Vec<&str> = (pre.as_array().expect("groups").iter())
        .map(|group| group["matcher"].as_str().unwrap_or("-"))
        .collect();
    assert_eq!(matchers, ["Grep|Glob|Bash", "Agent", "Write|Edit"]);
    assert_eq!(
        document["hooks"]["Stop"],
        json!([{"hooks": [{"type": "command", "command": "notify-send done"}]}])
    );
    assert!(text.contains("\"BIG\": 12345678901234567890"), "{text}");
    assert!(text.contains("\"NAME\": \"Z\\u00fcrich\""), "{text}");
    assert!(!text.contains("/old/bin"), "{text}");
    assert_eq!(rig.install(&[]).stdout.split(':').next(), Some("no change"));
}

#[test]
fn a_file_that_is_not_json_or_not_shaped_as_settings_is_refused_and_kept() {
    let rig = Rig::new();
    let shown = "<SANDBOX>/config/settings.json";
    for (written, stderr) in [
        (
            "{not json",
            format!(
                "agentless-mcp: {shown} is not valid JSON: Expecting property name enclosed in \
                 double quotes: line 1 column 2 (char 1). hook install wrote nothing.\n"
            ),
        ),
        (
            "[]",
            format!(
                "agentless-mcp: hooks: {shown} is not readable: $ is not an object; hook state \
                 unknown\n"
            ),
        ),
        (
            r#"{"hooks": []}"#,
            format!(
                "agentless-mcp: hooks: {shown} is not readable: $.hooks is not an object; hook \
                 state unknown\n"
            ),
        ),
        (
            r#"{"hooks": {"Stop": [{"hooks": [{"command": 7}]}]}}"#,
            format!(
                "agentless-mcp: hooks: {shown} is not readable: $.hooks.Stop[0].hooks[0].command \
                 is not a string; hook state unknown\n"
            ),
        ),
    ] {
        rig.put_settings(written);
        let output = rig.install(&[]);
        assert_eq!(rig.shown(&output), (1, String::new(), stderr), "{written}");
        assert_eq!(text(&rig.settings()), written);
    }
    assert!(
        !rig.sandbox
            .path()
            .join("config/settings.json.agentless-backup")
            .exists()
    );
}

// JSON.parse rejects NaN and Infinity, and json::parse reads 1e400 as Infinity.
#[test]
fn a_number_that_json_cannot_write_is_refused_and_kept() {
    let rig = Rig::new();
    let entry = r#""hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/a/agentless-mcp hook erosion"}]}]}"#;
    for number in ["1e400", "-1e400", "NaN", "[Infinity]"] {
        let written = format!("{{\"x\": {number}, {entry}}}");
        rig.put_settings(&written);
        for (output, command) in [
            (rig.install(&[]), "install"),
            (rig.install(&["--dry-run"]), "install"),
            (rig.uninstall(&[]), "uninstall"),
        ] {
            let stderr = format!(
                "agentless-mcp: <SANDBOX>/config/settings.json holds a number that JSON cannot \
                 write: NaN, Infinity, or a number past the 64-bit float range, such as 1e400. \
                 hook {command} wrote nothing.\n"
            );
            assert_eq!(rig.shown(&output), (1, String::new(), stderr), "{number}");
            assert_eq!(text(&rig.settings()), written);
        }
    }
    let backup = rig
        .sandbox
        .path()
        .join("config/settings.json.agentless-backup");
    assert!(!backup.exists());
}

#[test]
fn a_symlinked_settings_file_is_written_at_its_target() {
    let rig = Rig::new();
    drop(
        rig.sandbox
            .put("dotfiles/claude.json", "{\"model\": \"opus\"}\n"),
    );
    rig.sandbox
        .symlink("config/settings.json", "<SANDBOX>/dotfiles/claude.json");

    let output = rig.install(&[]);

    let wrote = format!("{FRESH}; wrote <SANDBOX>/dotfiles/claude.json\n");
    assert_eq!(rig.shown(&output), (0, wrote, String::new()));
    let link = std::fs::symlink_metadata(rig.settings()).expect("the link");
    assert!(link.file_type().is_symlink());
    let target = rig.sandbox.path().join("dotfiles/claude.json");
    assert!(text(&target).starts_with("{\n  \"model\": \"opus\",\n  \"hooks\": {\n"));
}

#[test]
fn the_write_is_atomic_and_the_first_backup_stays() {
    let rig = Rig::new();
    rig.put_settings("{\"model\": \"opus\"}");
    drop(rig.install(&[]));
    drop(rig.install(&["--server-name", "other"]));

    let mut names: Vec<String> = std::fs::read_dir(rig.sandbox.path().join("config"))
        .expect("the folder")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names, ["settings.json", "settings.json.agentless-backup"]);
    let backup = rig
        .sandbox
        .path()
        .join("config/settings.json.agentless-backup");
    assert_eq!(text(&backup), "{\"model\": \"opus\"}");
    assert!(text(&rig.settings()).contains("\"matcher\": \"mcp__other__trace\""));
}

// Design rule 4: the write keeps the mode of the file it replaces, and a new file gets 0600.
#[test]
fn install_and_uninstall_keep_the_files_mode_and_a_new_file_gets_0600() {
    let mode = |rig: &Rig| {
        let metadata = std::fs::metadata(rig.settings()).expect("the settings file");
        metadata.permissions().mode() & 0o777
    };
    let fresh = Rig::new();
    drop(fresh.install(&[]));
    assert_eq!(mode(&fresh), 0o600);

    let rig = Rig::new();
    rig.put_settings("{\"model\": \"opus\"}");
    rig.sandbox.set_mode("config/settings.json", 0o640);
    drop(rig.install(&[]));
    assert_eq!(mode(&rig), 0o640);
    drop(rig.uninstall(&[]));
    assert_eq!(mode(&rig), 0o640);
}

#[test]
fn a_file_that_changes_during_the_write_gets_one_retry_then_a_refusal() {
    let rig = Rig::new();
    rig.put_settings("{}");
    let target = rig.settings();
    let edit = |before: Option<&[u8]>| -> Result<(String, ()), ()> {
        let before = before.map_or_else(String::new, |bytes| {
            String::from_utf8_lossy(bytes).into_owned()
        });
        Ok((format!("{before}!"), ()))
    };
    let reads = Cell::new(0);
    let changing = |path: &Path| {
        reads.set(reads.get() + 1);
        disk::read(path).map(|bytes| bytes.map(|bytes| [bytes, vec![b'0' + reads.get()]].concat()))
    };
    let raced = disk::written(&target, &edit, &changing);
    assert!(matches!(raced, Ok((Written::Raced, ()))));
    assert_eq!(text(&target), "{}");

    let reads = Cell::new(0);
    let once = |path: &Path| {
        reads.set(reads.get() + 1);
        let changed = reads.get() == 2;
        disk::read(path)
            .map(|bytes| bytes.map(|bytes| if changed { b"{ }".to_vec() } else { bytes }))
    };
    let wrote = disk::written(&target, &edit, &once);
    assert!(matches!(wrote, Ok((Written::Wrote, ()))));
    assert_eq!(text(&target), "{}!");
}

#[test]
fn an_uninstall_that_races_names_uninstall() {
    let raced = uninstall_output(Ok((Written::Raced, Removed::default())), "/c/settings.json");
    let stderr = "agentless-mcp: /c/settings.json changed twice while hook uninstall wrote it. \
                  hook uninstall wrote nothing. Run it again.\n";
    assert_eq!((raced.code, raced.stderr.as_str()), (1, stderr));
}

#[test]
fn status_names_each_entry_and_exits_1_until_every_entry_is_current() {
    let rig = Rig::new();
    let missing = rig.status(&[]);
    rig.put_settings(MIXED);
    let old = rig.status(&[]);
    drop(rig.install(&[]));
    let current = rig.status(&[]);

    let names = [
        "read-card",
        "ledger-mark",
        "brief-record",
        "subagent-map",
        "prompt-map",
        "erosion",
        "ledger-reset",
    ];
    let lines = |state: &str| names.map(|name| format!("{name}: {state}\n")).concat();
    assert_eq!(rig.shown(&missing), (1, lines("missing"), String::new()));
    let old_lines = "read-card: missing\nledger-mark: missing\nbrief-record: missing\n\
                     subagent-map: missing\nprompt-map: missing\nerosion: runs \
                     /old/bin/agentless-mcp, not this binary\nledger-reset: missing\n\
                     gate-check: retired: hook install removes it\n\
                     gate-mark: retired: hook install removes it\n";
    assert_eq!(rig.shown(&old), (1, old_lines.to_owned(), String::new()));
    assert_eq!(rig.shown(&current), (0, lines("current"), String::new()));
}

#[test]
fn status_compares_the_server_name_that_it_is_given() {
    let rig = Rig::new();
    drop(rig.install(&["--server-name", "tools"]));
    let named = rig.status(&["--server-name", "tools"]);
    let default = rig.status(&[]);

    assert_eq!(named.code, 0, "{}", named.stdout);
    let stale = "ledger-mark: stale: the entry differs from the table of this binary\n";
    assert_eq!(default.code, 1);
    assert!(default.stdout.contains(stale), "{}", default.stdout);
}

#[test]
fn status_reads_the_project_and_local_files_and_reports_one_it_cannot_read() {
    let rig = Rig::new();
    drop(rig.install(&["--scope", "project"]));
    let project = rig.status(&[]);
    drop(rig.sandbox.put("repo/.claude/settings.local.json", "{oops"));
    let broken = rig.status(&[]);

    assert_eq!(project.code, 0, "{}", project.stdout);
    let first = "hooks: <SANDBOX>/repo/.claude/settings.local.json is not readable: Expecting \
                 property name enclosed in double quotes: line 1 column 2 (char 1); hook state \
                 unknown\n";
    let (code, stdout, _) = rig.shown(&broken);
    assert_eq!(code, 1);
    assert!(stdout.starts_with(first), "{stdout}");
    assert!(stdout.ends_with("ledger-reset: current\n"), "{stdout}");
}

#[test]
fn each_scope_writes_its_own_file() {
    let rig = Rig::new();
    for (scope, file) in [
        ("user", "config/settings.json"),
        ("project", "repo/.claude/settings.json"),
        ("local", "repo/.claude/settings.local.json"),
    ] {
        let output = rig.install(&["--scope", scope]);
        let expected = format!("{FRESH}; wrote <SANDBOX>/{file}\n");
        assert_eq!(rig.shown(&output), (0, expected, String::new()), "{scope}");
    }
    let elsewhere = rig.sandbox.mkdir("other");
    let output = rig.install(&[
        "--scope=project",
        "--repo",
        &elsewhere.display().to_string(),
    ]);
    assert!(
        output.stdout.ends_with("/other/.claude/settings.json\n"),
        "{}",
        output.stdout
    );
}

#[test]
fn uninstall_removes_the_agentless_entries_only() {
    let rig = Rig::new();
    rig.put_settings(MIXED);
    drop(rig.install(&[]));

    let first = rig.uninstall(&[]);
    let second = rig.uninstall(&[]);

    let report = "removed 7 agentless handlers, kept 2 other handlers; wrote \
                  <SANDBOX>/config/settings.json\n";
    assert_eq!(rig.shown(&first), (0, report.to_owned(), String::new()));
    assert_eq!(rig.shown(&second), (0, NOTHING.to_owned(), String::new()));
    let document: Value = serde_json::from_str(&text(&rig.settings())).expect("JSON");
    let expected = json!({
        "PreToolUse": [{"matcher": "Grep|Glob|Bash", "hooks": [{"type": "command", "command": "/usr/local/bin/audit-shell"}]}],
        "Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}],
    });
    assert_eq!(document["hooks"], expected);

    rig.put_settings(&example());
    let all = rig.uninstall(&[]);
    assert!(
        all.stdout
            .starts_with("removed 7 agentless handlers, kept 0 other handlers")
    );
    assert_eq!(text(&rig.settings()), "{}\n");
}

const NOTHING: &str =
    "nothing to remove: <SANDBOX>/config/settings.json holds no agentless entry\n";

// Uninstall is idempotent: a missing file, or one with no agentless entry, takes no write.
#[test]
fn uninstall_with_nothing_to_remove_reports_it_and_writes_nothing() {
    let rig = Rig::new();
    let missing = rig.uninstall(&[]);
    assert!(!rig.settings().exists());
    let compact = r#"{"model": "opus", "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}]}}"#;
    rig.put_settings(compact);
    let other = rig.uninstall(&[]);

    assert_eq!(rig.shown(&missing), (0, NOTHING.to_owned(), String::new()));
    assert_eq!(rig.shown(&other), (0, NOTHING.to_owned(), String::new()));
    assert_eq!(text(&rig.settings()), compact);
    let backup = rig
        .sandbox
        .path()
        .join("config/settings.json.agentless-backup");
    assert!(!backup.exists());
}

#[test]
fn uninstall_refuses_a_file_that_is_not_json_or_not_shaped_as_settings() {
    let rig = Rig::new();
    let shown = "<SANDBOX>/config/settings.json";
    for (written, stderr) in [
        (
            "{oops",
            format!(
                "agentless-mcp: {shown} is not valid JSON: Expecting property name enclosed in \
                 double quotes: line 1 column 2 (char 1). hook uninstall wrote nothing.\n"
            ),
        ),
        (
            r#"{"hooks": {"Stop": [1]}}"#,
            format!(
                "agentless-mcp: hooks: {shown} is not readable: $.hooks.Stop[0] is not an \
                 object; hook state unknown\n"
            ),
        ),
    ] {
        rig.put_settings(written);
        let output = rig.uninstall(&[]);
        assert_eq!(rig.shown(&output), (1, String::new(), stderr), "{written}");
        assert_eq!(text(&rig.settings()), written);
    }
}

#[test]
fn status_names_the_json_path_of_a_hooks_value_of_another_shape() {
    let rig = Rig::new();
    drop(
        rig.sandbox
            .put("repo/.claude/settings.json", r#"{"hooks": {"Stop": [1]}}"#),
    );
    let (code, stdout, _) = rig.shown(&rig.status(&[]));
    let first = "hooks: <SANDBOX>/repo/.claude/settings.json is not readable: $.hooks.Stop[0] is \
                 not an object; hook state unknown\n";
    assert_eq!(code, 1);
    assert!(stdout.starts_with(first), "{stdout}");
}

#[test]
fn an_option_that_a_command_does_not_take_is_a_usage_error() {
    let rig = Rig::new();
    let cases = [
        (
            rig.install(&["--force"]),
            "hook install has no parameter --force.",
        ),
        (
            rig.install(&["extra"]),
            "hook install has no parameter extra.",
        ),
        (
            rig.install(&["--scope", "team"]),
            "--scope is team, but must be one of user, project or local.",
        ),
        (
            rig.install(&["--repo"]),
            "--repo is required and must be a string.",
        ),
        (
            rig.install(&["--server-name="]),
            "--server-name is required and must be a string.",
        ),
        (
            rig.install(&["--dry-run=yes"]),
            "hook install has no parameter --dry-run=yes.",
        ),
        (
            rig.status(&["--scope", "user"]),
            "hook status has no parameter --scope.",
        ),
        (
            rig.uninstall(&["--dry-run"]),
            "hook uninstall has no parameter --dry-run.",
        ),
    ];
    for (output, message) in cases {
        let expected = (2, String::new(), format!("agentless-mcp: {message}\n"));
        assert_eq!(rig.shown(&output), expected);
    }
    assert!(!rig.settings().exists());
}

// Claude Code reads the ledger-mark matcher as a regular expression: `a|b` would match `b__trace`.
#[test]
fn a_server_name_with_a_character_outside_the_name_set_is_a_usage_error() {
    let rig = Rig::new();
    for name in ["a|b", "a.*", "a b", "na\u{ef}ve", "x(y)"] {
        for output in [
            rig.install(&["--server-name", name]),
            rig.status(&["--server-name", name]),
        ] {
            let message = format!(
                "agentless-mcp: --server-name is {name}, but must be a name of ASCII letters, \
                 digits, _ and -.\n"
            );
            assert_eq!(rig.shown(&output), (2, String::new(), message), "{name}");
        }
    }
    assert!(!rig.settings().exists());
    let output = rig.install(&["--server-name=Tools_2-b"]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert!(text(&rig.settings()).contains("\"matcher\": \"mcp__Tools_2-b__trace\""));
}

#[test]
fn the_layout_is_two_space_json_in_key_order() {
    let value =
        json::parse(r#"{"b": [], "a": {}, "c": [1, {"x": null}], "d": "q\"\u00e9", "e": 1.5}"#)
            .expect("JSON");
    let expected = "{\n  \"b\": [],\n  \"a\": {},\n  \"c\": [\n    1,\n    {\n      \"x\": null\n    \
                    }\n  ],\n  \"d\": \"q\\\"\\u00e9\",\n  \"e\": 1.5\n}\n";
    assert_eq!(indented(&value).as_deref(), Ok(expected));
    assert_eq!(
        indented(&json::JsonValue::Object(Vec::new())).as_deref(),
        Ok("{}\n")
    );
}
