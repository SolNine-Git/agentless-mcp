use std::ffi::OsString;
use std::path::{Path, PathBuf};

use app::EnvConfig;
use app::hooks::{DEFAULT_SERVER_NAME, Hook, command, entries};
use io::testing::Tree;

use super::{HookCheck, Hooks, SessionNote};

const ALL_MISSING: &str = "hooks: 7 of 7 agentless hook entries are missing (read-card, \
     ledger-mark, brief-record, subagent-map, prompt-map, erosion, ledger-reset). Run: \
     agentless-mcp hook install";

// The settings of the 0.11.0 hooks, contrib/hooks/settings-example.json with `BINARY` for its
// path.
const OLD_RELEASE: &str = r#"{"model": "x", "hooks": {
  "PreToolUse": [
    {"matcher": "Grep|Glob|Bash", "hooks": [
      {"type": "command", "command": "BINARY hook gate-check"}]},
    {"matcher": "Write|Edit", "hooks": [
      {"type": "command", "command": "BINARY hook erosion", "timeout": 15}]}
  ],
  "PostToolUse": [
    {"matcher": "mcp__agentless__.*", "hooks": [
      {"type": "command", "command": "BINARY hook gate-mark"}]}
  ],
  "UserPromptSubmit": [
    {"hooks": [{"type": "command", "command": "BINARY hook prompt-map", "timeout": 15}]}
  ]
}}"#;

// A home with a user settings folder, a repository root, and a stand-in for the server's binary.
struct Rig {
    tree: Tree,
    binary: PathBuf,
    root: PathBuf,
}

impl Rig {
    fn new() -> Rig {
        let tree = Tree::new().expect("a scratch directory");
        tree.create_dir("config").expect("a config folder");
        tree.create_dir("repo/.claude").expect("a project folder");
        tree.create_dir("bin").expect("a binary folder");
        let binary = tree.write_file("bin/agentless-mcp", "").expect("a binary");
        Rig {
            binary: io::resolve(&binary).expect("a resolved binary"),
            root: tree.join("repo"),
            tree,
        }
    }

    fn check(&self) -> HookCheck {
        HookCheck {
            user_settings: Some(self.tree.join("config/settings.json")),
            binary: self.binary.clone(),
        }
    }

    fn write(&self, relative: &str, text: &str) {
        let text = text.replace("BINARY", &self.binary.display().to_string());
        self.tree
            .write_file(relative, text)
            .expect("a settings file");
    }

    fn note(&self, hooks: &Hooks) -> Option<String> {
        hooks.note(&self.root)
    }
}

// The `hooks` value that `hook install` writes for `hooks`, run from `binary`.
fn installed(binary: &str, hooks: &[Hook]) -> String {
    let groups: Vec<String> = (entries(DEFAULT_SERVER_NAME).into_iter())
        .filter(|entry| hooks.contains(&entry.hook))
        .map(|entry| {
            let matcher = (entry.matcher.as_ref())
                .map_or(String::new(), |m| format!("\"matcher\": \"{m}\", "));
            let handler = format!(
                "{{\"type\": \"command\", \"command\": \"{}\", \"timeout\": {}}}",
                command(binary, entry.hook),
                entry.timeout
            );
            format!(
                "\"{}\": [{{{matcher}\"hooks\": [{handler}]}}]",
                entry.event.as_str()
            )
        })
        .collect();
    // Each event key holds one group per hook, so the groups of one event merge into one key.
    let mut merged: Vec<(String, Vec<String>)> = Vec::new();
    for group in groups {
        let (key, rest) = group.split_once(": ").expect("a key");
        let rest = rest
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        match merged.iter_mut().find(|(known, _)| known == key) {
            Some((_, list)) => list.push(rest),
            None => merged.push((key.to_owned(), vec![rest])),
        }
    }
    let body: Vec<String> = (merged.iter())
        .map(|(key, list)| format!("{key}: [{}]", list.join(", ")))
        .collect();
    format!("{{\"hooks\": {{{}}}}}", body.join(", "))
}

#[test]
fn the_user_settings_file_follows_the_config_dir_then_the_home() {
    let env = |vars: &[(&str, &str)]| {
        let vars = vars
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        EnvConfig::new(vars, PathBuf::from("/"))
    };
    let binary = PathBuf::from("/b/agentless-mcp");
    let of = |vars: &[(&str, &str)]| HookCheck::of(&env(vars), binary.clone()).user_settings;
    let named = [("CLAUDE_CONFIG_DIR", "/c"), ("HOME", "/h")];
    assert_eq!(of(&named), Some(PathBuf::from("/c/settings.json")));
    let empty = [("CLAUDE_CONFIG_DIR", ""), ("HOME", "/h")];
    assert_eq!(of(&empty), Some(PathBuf::from("/h/.claude/settings.json")));
    assert_eq!(of(&[]), None);
}

#[test]
fn no_check_gives_no_note_and_no_file_gives_every_entry_missing() {
    let rig = Rig::new();
    assert_eq!(rig.note(&Hooks::new(None, &[])), None);
    let hooks = Hooks::new(Some(rig.check()), std::slice::from_ref(&rig.root));
    assert_eq!(rig.note(&hooks).as_deref(), Some(ALL_MISSING));
}

#[test]
fn the_entries_of_this_binary_in_the_user_and_project_files_are_current() {
    let rig = Rig::new();
    let binary = rig.binary.display().to_string();
    let (user, project) = Hook::ALL.split_at(3);
    let (project, local) = project.split_at(2);
    rig.write("config/settings.json", &installed(&binary, user));
    rig.write("repo/.claude/settings.json", &installed(&binary, project));
    rig.write(
        "repo/.claude/settings.local.json",
        &installed(&binary, local),
    );
    let hooks = Hooks::new(Some(rig.check()), &[]);
    assert_eq!(rig.note(&hooks), None);
}

// The design's example: the 0.11.0 entries keep two current hooks and two retired ones.
#[test]
fn the_entries_of_the_last_release_are_missing_and_retired() {
    let rig = Rig::new();
    rig.write("config/settings.json", OLD_RELEASE);
    let hooks = Hooks::new(Some(rig.check()), &[]);
    let want = "hooks: 5 of 7 agentless hook entries are missing (read-card, ledger-mark, \
                brief-record, subagent-map, ledger-reset) and 2 are retired (gate-check, \
                gate-mark). Run: agentless-mcp hook install";
    assert_eq!(rig.note(&hooks).as_deref(), Some(want));
}

#[test]
fn a_changed_entry_is_stale_and_a_retired_one_is_named_once() {
    let rig = Rig::new();
    let binary = rig.binary.display().to_string();
    let current = installed(&binary, &Hook::ALL).replace("\"timeout\": 15}", "\"timeout\": 10}");
    rig.write("config/settings.json", &current);
    let retired = OLD_RELEASE.replace("hook erosion", "hook gate-check");
    rig.write("repo/.claude/settings.local.json", &retired);
    let hooks = Hooks::new(Some(rig.check()), &[]);
    let want = "hooks: 3 of 7 agentless hook entries are stale (subagent-map, prompt-map, \
                erosion) and 2 are retired (gate-check, gate-mark). Run: agentless-mcp hook \
                install";
    assert_eq!(rig.note(&hooks).as_deref(), Some(want));
}

// An entry that runs a link to this binary runs this binary; one that runs another file says so.
#[test]
fn an_entry_on_another_path_is_current_only_when_the_path_resolves_to_this_binary() {
    let rig = Rig::new();
    rig.tree.create_dir("alias").expect("a link folder");
    rig.tree
        .symlink("alias/agentless-mcp", &rig.binary)
        .expect("a link");
    let link = rig.tree.join("alias/agentless-mcp");
    let linked = installed(&link.display().to_string(), &Hook::ALL);
    rig.write("config/settings.json", &linked);
    let hooks = Hooks::new(Some(rig.check()), &[]);
    let other = rig.tree.join("other/agentless-mcp").display().to_string();
    rig.write("repo/.claude/settings.json", "{}");
    assert_eq!(rig.note(&hooks), None);
    rig.write("config/settings.json", &installed(&other, &Hook::ALL));
    let want = format!(
        "hooks run {other}; this server runs {}",
        rig.binary.display()
    );
    assert_eq!(rig.note(&hooks), Some(want));
}

// A file that cannot be read, that is not JSON, or whose hooks value has another shape makes the
// state unknown; the next read after a fix answers again.
#[test]
fn a_file_that_cannot_be_read_makes_the_state_unknown() {
    let rig = Rig::new();
    let path = rig.tree.join("repo/.claude/settings.json");
    let unknown = |error: &str| {
        Some(format!(
            "hooks: {} is not readable: {error}; hook state unknown",
            path.display()
        ))
    };
    let hooks = Hooks::new(Some(rig.check()), std::slice::from_ref(&rig.root));
    rig.tree
        .create_dir("repo/.claude/settings.json")
        .expect("a folder in the file's place");
    assert_eq!(rig.note(&hooks), unknown("[Errno 21] Is a directory"));
    io::remove_tree(&path).expect("a removed folder");
    for (text, error) in [
        (
            "{\"hooks\": ",
            "Expecting value: line 1 column 11 (char 10)",
        ),
        ("[]", "$ is not an object"),
        ("{\"hooks\": []}", "$.hooks is not an object"),
        (
            "{\"hooks\": {\"PreToolUse\": [{\"hooks\": [{\"timeout\": \"3\"}]}]}}",
            "$.hooks.PreToolUse[0].hooks[0].timeout is not a number",
        ),
    ] {
        rig.write("repo/.claude/settings.json", text);
        assert_eq!(rig.note(&hooks), unknown(error), "{text}");
    }
    rig.write("repo/.claude/settings.json", "{\"hooks\": {}}");
    assert_eq!(rig.note(&hooks).as_deref(), Some(ALL_MISSING));
}

// The server cannot know the name its client registered it under, so a trace matcher of any
// server name is current.
#[test]
fn a_ledger_mark_matcher_of_another_server_name_is_current() {
    let rig = Rig::new();
    let binary = rig.binary.display().to_string();
    let renamed =
        installed(&binary, &Hook::ALL).replace("mcp__agentless__trace", "mcp__tools__trace");
    rig.write("config/settings.json", &renamed);
    let hooks = Hooks::new(Some(rig.check()), std::slice::from_ref(&rig.root));
    assert_eq!(rig.note(&hooks), None);
}

// The read at start-up holds while a file's (mtime, size) holds; a change is read again.
#[test]
fn a_changed_file_is_read_again() {
    let rig = Rig::new();
    let binary = rig.binary.display().to_string();
    rig.write("config/settings.json", &installed(&binary, &Hook::ALL));
    let hooks = Hooks::new(Some(rig.check()), std::slice::from_ref(&rig.root));
    assert_eq!(rig.note(&hooks), None);
    rig.write(
        "config/settings.json",
        "{\"hooks\": {}, \"theme\": \"dark\"}",
    );
    assert_eq!(rig.note(&hooks).as_deref(), Some(ALL_MISSING));
    let path: &Path = &rig.tree.join("config/settings.json");
    io::remove_file(path).expect("a removed file");
    assert_eq!(rig.note(&hooks).as_deref(), Some(ALL_MISSING));
}

#[test]
fn a_session_note_is_claimed_once_and_a_new_session_claims_its_own() {
    let note = SessionNote::default();
    let shared = note.clone();
    assert!(note.claim());
    assert!(!shared.claim());
    assert!(SessionNote::default().claim());
}
