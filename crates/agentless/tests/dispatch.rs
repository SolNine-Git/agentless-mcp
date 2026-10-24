//! The binary: what it sends to the command line and to the hook, its exit codes, and a closed
//! stdout.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use io::testing::Tree;

const BINARY: &str = env!("CARGO_BIN_EXE_agentless-mcp");
const DEADLINE: Duration = Duration::from_secs(30);
const HOOK_REFUSAL: &str =
    "agentless-mcp: hook takes one argument: erosion, gate-check or gate-mark\n";
const GATE_LOG: &str = "AGENTLESS_GATE_LOG";
// The shell enters `gone`, removes it, and then becomes the hook, so the hook starts there.
const IN_REMOVED_DIRECTORY: &str = r#"cd gone && rmdir "$PWD" && exec "$0" hook "$1""#;

struct Ran {
    code: Option<i32>,
    out: String,
    err: String,
}

fn sandbox() -> Tree {
    let tree = Tree::new().unwrap_or_else(|error| panic!("a temporary tree: {error}"));
    tree.create_dir("home").expect("a home directory");
    tree
}

fn env(tree: &Tree, extra: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    let mut vars = io::testing::hermetic_env(&tree.path().join("home"));
    vars.extend(
        extra
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value))),
    );
    vars
}

fn run(tree: &Tree, args: &[&str], extra: &[(&str, &str)]) -> Ran {
    let spec = io::Spec {
        program: OsString::from(BINARY),
        args: args.iter().map(OsString::from).collect(),
        cwd: tree.path().to_path_buf(),
        env: env(tree, extra),
        timeout: DEADLINE,
        stdout_cap: 1 << 24,
        stderr_cap: 1 << 16,
    };
    let outcome = io::run(&spec);
    let io::Outcome::Exited {
        status,
        stdout,
        stderr,
    } = outcome
    else {
        panic!("the binary did not finish: {outcome:?}");
    };
    Ran {
        code: status.code(),
        out: String::from_utf8(stdout).expect("UTF-8 stdout"),
        err: String::from_utf8(stderr).expect("UTF-8 stderr"),
    }
}

#[test]
fn help_goes_to_stdout_and_exits_0() {
    let ran = run(&sandbox(), &["--help"], &[]);
    assert_eq!(ran.code, Some(0));
    assert!(
        ran.out.starts_with("usage: agentless-mcp [-h]"),
        "{}",
        ran.out
    );
    assert!(ran.err.is_empty(), "{}", ran.err);
}

#[test]
fn an_unknown_command_is_a_usage_error() {
    let ran = run(&sandbox(), &["teleport", "--repo", "r"], &[]);
    assert_eq!(ran.code, Some(2));
    assert!(ran.out.is_empty());
    assert!(
        ran.err
            .contains("error: argument command: invalid choice: 'teleport'"),
        "{}",
        ran.err
    );
}

#[test]
fn capabilities_lists_the_caps_in_force() {
    let tree = sandbox();
    let repo = tree.path().display().to_string();
    let ran = run(&tree, &["capabilities", "--repo", &repo], &[]);
    assert_eq!(ran.code, Some(0), "{}", ran.err);
    assert!(
        ran.out.contains("\n  max_output_tokens = 16000\n"),
        "{}",
        ran.out
    );
}

#[test]
fn a_hook_command_line_that_names_no_hook_is_refused() {
    let tree = sandbox();
    for args in [
        &["hook"][..],
        &["hook", "nope"],
        &["hook", "erosion", "extra"],
        &["hook", "gate-check", "extra"],
        &["hook", "gate-mark", "extra"],
    ] {
        let ran = run(&tree, args, &[]);
        assert_eq!(
            (ran.code, ran.out.as_str(), ran.err.as_str()),
            (Some(2), "", HOOK_REFUSAL)
        );
    }
}

// Runs `command` with `payload` on stdin and only the hermetic variables and `extra`, so no gate
// log of the caller reaches the hook.
fn fed(mut command: Command, tree: &Tree, payload: &str, extra: &[(&str, &str)]) -> Ran {
    let mut child = command
        .current_dir(tree.path())
        .env_clear()
        .envs(env(tree, extra))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("a started binary");
    let mut stdin = child.stdin.take().expect("a stdin pipe");
    stdin
        .write_all(payload.as_bytes())
        .expect("the payload is written");
    drop(stdin);
    let status = wait(&mut child);
    let (mut out, mut err) = (String::new(), String::new());
    let mut stdout = child.stdout.take().expect("a stdout pipe");
    stdout.read_to_string(&mut out).expect("UTF-8 stdout");
    let mut stderr = child.stderr.take().expect("a stderr pipe");
    stderr.read_to_string(&mut err).expect("UTF-8 stderr");
    Ran {
        code: status.code(),
        out,
        err,
    }
}

fn gate(tree: &Tree, hook: &str, payload: &str, extra: &[(&str, &str)]) -> Ran {
    let mut command = Command::new(BINARY);
    command.args(["hook", hook]);
    fed(command, tree, payload, extra)
}

// The one log line without its ts, which the run's own clock sets.
fn after_ts(log: &str) -> String {
    let rest = log
        .strip_prefix("{\"ts\": ")
        .unwrap_or_else(|| panic!("a line that starts with the ts: {log}"));
    let (ts, tail) = rest
        .split_once(", ")
        .unwrap_or_else(|| panic!("a field after the ts: {log}"));
    assert!(ts.contains('.') && ts.parse::<f64>().is_ok(), "{log}");
    tail.to_owned()
}

#[test]
fn hook_gate_check_reaches_the_search_gate() {
    let tree = sandbox();
    let log = tree.join("gate.jsonl");
    let shown = log.to_str().expect("a UTF-8 log path");
    let payload = r#"{"session_id": "s", "tool_name": "Read"}"#;

    let ran = gate(&tree, "gate-check", payload, &[(GATE_LOG, shown)]);

    let quiet = (Some(0), "", "");
    assert_eq!((ran.code, ran.out.as_str(), ran.err.as_str()), quiet);
    let line = std::fs::read_to_string(&log).expect("the gate log");
    let tail = concat!(
        r#""session_id": "s", "tool": "Read", "pattern": null, "#,
        r#""event": "native_search_allowed", "reason": "not_a_search_tool"}"#,
    );
    assert_eq!(after_ts(&line), format!("{tail}\n"));
}

#[test]
fn hook_gate_mark_reaches_the_search_gate() {
    let tree = sandbox();
    let log = tree.join("gate.jsonl");
    let shown = log.to_str().expect("a UTF-8 log path");
    let payload = r#"{"session_id": "s", "tool_name": "mcp__agentless__capabilities"}"#;

    let ran = gate(&tree, "gate-mark", payload, &[(GATE_LOG, shown)]);

    let quiet = (Some(0), "", "");
    assert_eq!((ran.code, ran.out.as_str(), ran.err.as_str()), quiet);
    let line = std::fs::read_to_string(&log).expect("the gate log");
    let tail = concat!(
        r#""session_id": "s", "event": "agentless_call_ignored", "#,
        r#""tool": "mcp__agentless__capabilities", "operation": null, "#,
        r#""reason": "non_localizing_operation"}"#,
    );
    assert_eq!(after_ts(&line), format!("{tail}\n"));
}

// The gate hooks come before the start-up directory test of the other command lines.
#[test]
fn the_gate_hooks_run_in_a_removed_directory() {
    let tree = sandbox();
    let log = tree.join("gate.jsonl");
    let shown = log.to_str().expect("a UTF-8 log path");
    for hook in ["gate-check", "gate-mark"] {
        let mut command = Command::new("sh");
        command.args(["-c", IN_REMOVED_DIRECTORY, BINARY, hook]);
        tree.create_dir("gone").expect("the start directory");

        let ran = fed(command, &tree, "{}", &[(GATE_LOG, shown)]);

        let quiet = (Some(0), "", "");
        assert_eq!(
            (ran.code, ran.out.as_str(), ran.err.as_str()),
            quiet,
            "{hook}"
        );
    }
    let line = std::fs::read_to_string(&log).expect("the gate log");
    let tail = concat!(
        r#""session_id": null, "tool": null, "pattern": null, "#,
        r#""event": "native_search_allowed", "reason": "no_session_id"}"#,
    );
    assert_eq!(after_ts(&line), format!("{tail}\n"));
}

#[test]
fn hook_erosion_reaches_the_hook() {
    let ran = run(
        &sandbox(),
        &["hook", "erosion"],
        &[("AGENTLESS_EROSION_GATE", "off")],
    );
    assert_eq!(
        (ran.code, ran.out.as_str(), ran.err.as_str()),
        (Some(0), "", "")
    );
}

// Waits for `child` with a deadline, and kills it when the deadline passes.
fn wait(child: &mut Child) -> ExitStatus {
    let start = Instant::now();
    while start.elapsed() < DEADLINE {
        if let Some(status) = child.try_wait().expect("a child status") {
            return status;
        }
        thread::sleep(Duration::from_millis(10));
    }
    child.kill().expect("a killed child");
    panic!("the binary did not finish in {DEADLINE:?}");
}

#[test]
fn a_closed_stdout_exits_120_with_two_lines() {
    let tree = sandbox();
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);
    let mut child = Command::new(BINARY)
        .arg("--help")
        .current_dir(tree.path())
        .env_clear()
        .envs(env(&tree, &[]))
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn()
        .expect("a started binary");
    let status = wait(&mut child);
    let mut err = String::new();
    let mut stderr = child.stderr.take().expect("a stderr pipe");
    stderr.read_to_string(&mut err).expect("UTF-8 stderr");
    assert_eq!(status.code(), Some(120));
    let lines =
        "Exception ignored on flushing sys.stdout:\nBrokenPipeError: [Errno 32] Broken pipe\n";
    assert_eq!(err, lines);
}
