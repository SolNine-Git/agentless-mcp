use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use app::{EnvConfig, Services};
use serde_json::{Value, json};

use super::{ENV_IGNORE, ENV_LOG, run};
use crate::testing::{Sandbox, Steps};
use crate::{ENV_GATE, Output};

const LONG_COMMENT: &str = "# one\n# two\n# three\nx = 1\n";
const WRITE: &str = "cat > a.py <<EOF";
const WRITE_CLOSING: &str = "This write skipped the Write/Edit gate. Fix these now.";
const STOP_CLOSING: &str = "Fix these before you end the turn.";

// A repository on main with one clean Python file, a branch that has no change yet, and an
// environment whose temporary folder, home and log are the sandbox's own.
struct Scene {
    sandbox: Sandbox,
    env: EnvConfig,
    services: Services,
}

impl Scene {
    fn new() -> Self {
        Self::with(&[])
    }

    fn with(extra: &[(&str, String)]) -> Self {
        Self::built(extra, true)
    }

    // A repository that has no commit, so that no revision resolves.
    fn unborn() -> Self {
        Self::built(&[], false)
    }

    fn built(extra: &[(&str, String)], committed: bool) -> Self {
        let sandbox = Sandbox::new();
        sandbox.mkdir("repo");
        sandbox.mkdir("tmp");
        sandbox.git(&["init", "--quiet", "-b", "main"]);
        sandbox.put("repo/a.py", "x = 0\n");
        if committed {
            sandbox.git(&["add", "-A"]);
            sandbox.git(&["commit", "--quiet", "-m", "fixture"]);
            sandbox.git(&["checkout", "--quiet", "-b", "work"]);
        }
        let mut vars: Vec<(&str, OsString)> = vec![
            (
                "GIT_CEILING_DIRECTORIES",
                sandbox.path().as_os_str().to_owned(),
            ),
            ("TMPDIR", sandbox.path().join("tmp").into_os_string()),
            (ENV_LOG, sandbox.path().join("scan.jsonl").into_os_string()),
        ];
        vars.extend(
            extra
                .iter()
                .map(|(name, value)| (*name, OsString::from(value))),
        );
        let env = sandbox.env(&vars);
        let services = Services::new(&env);
        Self {
            sandbox,
            env,
            services,
        }
    }

    fn call(&self, payload: &Value) -> Output {
        let text = payload.to_string();
        let mut stdin = text.as_bytes();
        run(&self.env, &self.services, &mut stdin, &Steps::fixed())
    }

    fn post(&self, command: &str) -> Output {
        self.call(&json!({
            "hook_event_name": "PostToolUse",
            "session_id": "s1",
            "cwd": self.sandbox.root(),
            "tool_input": {"command": command},
        }))
    }

    fn stop(&self, event: &str, active: bool) -> Output {
        self.call(&json!({
            "hook_event_name": event,
            "session_id": "s1",
            "cwd": self.sandbox.root(),
            "stop_hook_active": active,
        }))
    }

    fn write(&self, text: &str) {
        self.sandbox.put("repo/a.py", text);
    }

    fn records(&self) -> Vec<Value> {
        let path = self.sandbox.path().join("scan.jsonl");
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
            Err(error) => panic!("the scan log does not read: {error}"),
        };
        text.lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect()
    }

    fn reasons(&self) -> Vec<String> {
        let records = self.records();
        let reasons = records.iter().map(|record| record["reason"].as_str());
        reasons
            .map(|reason| reason.expect("a reason").to_owned())
            .collect()
    }
}

#[test]
fn a_shell_write_that_adds_a_long_comment_exits_2_with_the_block_layout() {
    let scene = Scene::new();
    scene.write(LONG_COMMENT);

    let output = scene.post(WRITE);

    assert_eq!(output.code, 2);
    assert_eq!(output.stdout, "");
    let lines: Vec<&str> = output.stderr.lines().collect();
    assert_eq!(lines[0], "Erosion gate blocked this edit:");
    assert_eq!(
        lines[1],
        "repo/a.py:1: 3-line comment; why-comments run 1-2 lines"
    );
    assert_eq!(lines.last(), Some(&WRITE_CLOSING));
    assert!(
        output
            .stderr
            .contains("Delete the flagged text, then retry the edit.")
    );
    assert!(!output.stderr.contains("Erosion gate mode"));
}

#[test]
fn the_same_finding_is_reported_once_after_a_write() {
    let scene = Scene::new();
    scene.write(LONG_COMMENT);

    let first = scene.post(WRITE);
    let again = scene.post(WRITE);

    assert_eq!(first.code, 2);
    assert_eq!(again, Output::default());
    assert_eq!(scene.reasons(), ["reported", "clean"]);
}

#[test]
fn a_new_finding_in_a_later_write_is_reported_and_the_old_one_is_not() {
    let scene = Scene::new();
    scene.write(LONG_COMMENT);
    scene.post(WRITE);
    scene.write("# one\n# two\n# three\nx = 1\n# four\n# five\n# six\ny = 2\n");

    let output = scene.post(WRITE);

    assert_eq!(output.code, 2);
    assert_eq!(
        output.stderr.matches("-line comment").count(),
        1,
        "{}",
        output.stderr
    );
    assert!(output.stderr.contains("repo/a.py:5:"), "{}", output.stderr);
}

#[test]
fn a_stop_after_a_write_reports_every_finding_still_on_disk_even_one_already_reported() {
    let scene = Scene::new();
    scene.write(LONG_COMMENT);
    scene.post(WRITE);

    let stopped = scene.stop("Stop", false);
    let again = scene.stop("SubagentStop", false);

    assert_eq!(stopped.code, 2);
    assert_eq!(stopped.stderr.lines().last(), Some(STOP_CLOSING));
    assert!(stopped.stderr.contains("repo/a.py:1: 3-line comment"));
    assert_eq!(again, stopped);
}

#[test]
fn a_stop_with_stop_hook_active_exits_0_and_scans_nothing() {
    let scene = Scene::new();
    scene.write(LONG_COMMENT);
    scene.post(WRITE);

    let output = scene.stop("Stop", true);

    assert_eq!(output, Output::default());
    assert_eq!(scene.reasons(), ["reported", "stop_hook_active"]);
}

#[test]
fn a_stop_ends_clean_once_the_text_is_fixed() {
    let scene = Scene::new();
    scene.write(LONG_COMMENT);
    scene.post(WRITE);
    scene.write("# one\nx = 1\n");

    let output = scene.stop("Stop", false);

    assert_eq!(output, Output::default());
    assert_eq!(scene.reasons(), ["reported", "clean"]);
}

#[test]
fn a_stop_scans_only_the_roots_that_a_shell_write_marked() {
    let scene = Scene::new();
    scene.write(LONG_COMMENT);

    let output = scene.stop("Stop", false);

    assert_eq!(output, Output::default());
    assert_eq!(scene.reasons(), ["no_roots"]);
}

#[test]
fn a_command_that_does_not_write_is_not_scanned() {
    let scene = Scene::new();
    scene.write(LONG_COMMENT);

    let output = scene.post("git status && cat a.py");

    assert_eq!(output, Output::default());
    assert_eq!(scene.reasons(), ["not_a_write"]);
    assert_eq!(scene.stop("Stop", false), Output::default());
}

#[test]
fn a_finding_that_the_base_already_holds_is_not_reported() {
    let scene = Scene::new();
    scene.sandbox.git(&["checkout", "--quiet", "main"]);
    scene.write(LONG_COMMENT);
    scene
        .sandbox
        .git(&["commit", "--quiet", "-am", "inherited"]);
    scene.sandbox.git(&["checkout", "--quiet", "-b", "later"]);
    scene.write("# one\n# two\n# three\nx = 2\n");

    let output = scene.post(WRITE);

    assert_eq!(output, Output::default());
    assert_eq!(scene.reasons(), ["clean"]);
}

#[test]
fn a_repository_that_sets_the_check_to_advisory_is_not_scanned() {
    let scene = Scene::new();
    scene.sandbox.put(
        "repo/.agentless-mcp.json",
        r#"{"erosion_gate": {"comment_delta": "advisory"}}"#,
    );
    scene.write(LONG_COMMENT);

    let output = scene.post(WRITE);

    assert_eq!(output, Output::default());
    assert_eq!(scene.reasons(), ["clean"]);
}

#[test]
fn the_environment_mode_off_turns_the_hook_off_with_no_record() {
    let scene = Scene::with(&[(ENV_GATE, "off".to_owned())]);
    scene.write(LONG_COMMENT);

    assert_eq!(scene.post(WRITE), Output::default());
    assert_eq!(scene.stop("Stop", false), Output::default());
    assert_eq!(scene.records(), Vec::<Value>::new());
}

#[test]
fn an_ignored_root_is_neither_marked_nor_scanned() {
    let scene = Scene::new();
    let root = io::resolve(&scene.sandbox.root()).expect("a resolved root");
    let env = scene.sandbox.env(&[
        (
            "GIT_CEILING_DIRECTORIES",
            scene.sandbox.path().as_os_str().to_owned(),
        ),
        ("TMPDIR", scene.sandbox.path().join("tmp").into_os_string()),
        (
            ENV_LOG,
            scene.sandbox.path().join("scan.jsonl").into_os_string(),
        ),
        (ENV_IGNORE, root.into_os_string()),
    ]);
    let services = Services::new(&env);
    scene.write(LONG_COMMENT);
    let payload = json!({
        "hook_event_name": "PostToolUse",
        "session_id": "s1",
        "cwd": scene.sandbox.root(),
        "tool_input": {"command": WRITE},
    })
    .to_string();

    let output = run(&env, &services, &mut payload.as_bytes(), &Steps::fixed());

    assert_eq!(output, Output::default());
    assert_eq!(scene.reasons(), ["no_roots"]);
}

#[test]
fn a_payload_with_a_missing_field_exits_0_and_is_logged() {
    let scene = Scene::new();

    let output = scene.call(&json!({"hook_event_name": "Stop", "session_id": "s1", "cwd": "/w"}));

    assert_eq!(output, Output::default());
    assert_eq!(
        scene.reasons(),
        ["malformed payload: payload.stop_hook_active is missing"]
    );
    assert_eq!(scene.records()[0]["event"], Value::Null);
}

#[test]
fn a_record_names_the_event_the_session_and_the_counts() {
    let scene = Scene::new();
    scene.write(LONG_COMMENT);

    scene.post(WRITE);

    let records = scene.records();
    let [record] = records.as_slice() else {
        panic!("one record: {records:?}");
    };
    assert_eq!(record["hook"], "erosion-scan");
    assert_eq!(record["event"], "PostToolUse");
    assert_eq!(record["session_id"], "s1");
    assert_eq!(record["roots"], 1);
    assert_eq!(record["findings"], 1);
    assert_eq!(record["error"], Value::Null);
}

#[test]
fn more_than_fifteen_findings_list_fifteen_and_count_the_rest() {
    let scene = Scene::new();
    let blocks: Vec<String> = (0..17)
        .map(|index| format!("# a{index}\n# b{index}\n# c{index}\nx{index} = 1\n"))
        .collect();
    let text = blocks.concat();
    scene.write(&text);

    let output = scene.post(WRITE);

    assert_eq!(output.code, 2);
    assert_eq!(output.stderr.matches("-line comment").count(), 15);
    assert!(output.stderr.contains("...and 2 more findings"));
}

#[test]
fn the_state_lives_below_the_temporary_folder() {
    let scene = Scene::new();
    scene.write(LONG_COMMENT);
    scene.post(WRITE);

    let state: PathBuf = scene.sandbox.path().join("tmp").join("agentless_scan");
    assert!(state.is_dir());
}

#[test]
fn a_root_that_cannot_be_scanned_is_logged_as_degraded_and_exits_0() {
    let scene = Scene::unborn();
    scene.write(LONG_COMMENT);

    let output = scene.post(WRITE);

    assert_eq!(output, Output::default());
    assert_eq!(scene.reasons(), ["degraded"]);
    let error = scene.records()[0]["error"]
        .as_str()
        .expect("an error")
        .to_owned();
    assert!(
        error.contains("--since HEAD does not name a commit"),
        "{error}"
    );
}
