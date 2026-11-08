//! The binary: what it sends to the command line and to the hook, its exit codes, and a closed
//! stdout.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use io::testing::{Identity, Tree};

const BINARY: &str = env!("CARGO_BIN_EXE_agentless-mcp");
const DEADLINE: Duration = Duration::from_secs(30);
const HOOK_REFUSAL: &str =
    "agentless-mcp: hook takes one argument: erosion, gate-check, gate-mark or prompt-map\n";
const GATE_LOG: &str = "AGENTLESS_GATE_LOG";
// The shell enters `gone`, removes it, and then becomes the hook, so the hook starts there.
const IN_REMOVED_DIRECTORY: &str = r#"cd gone && rmdir "$PWD" && exec "$0" hook "$1""#;
const IDENTITY: Identity = Identity {
    name: "hook tests",
    email: "hook@example.invalid",
    date: "2026-02-01T00:00:00Z",
};

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
        &["hook", "prompt-map", "extra"],
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
fn fed(command: Command, tree: &Tree, payload: &str, extra: &[(&str, &str)]) -> Ran {
    finished(started(command, tree, payload, extra))
}

// Starts `command` and hands it `payload` on a closed stdin.
fn started(mut command: Command, tree: &Tree, payload: &str, extra: &[(&str, &str)]) -> Child {
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
    child
}

fn finished(mut child: Child) -> Ran {
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

// The prompt map also comes before the start-up directory test, and an empty stdin is silent.
#[test]
fn hook_prompt_map_on_an_empty_stdin_in_a_removed_directory_writes_nothing() {
    let tree = sandbox();
    tree.create_dir("gone").expect("the start directory");
    let tmp = tree.path().display().to_string();
    let mut command = Command::new("sh");
    command.args(["-c", IN_REMOVED_DIRECTORY, BINARY, "prompt-map"]);

    let ran = fed(command, &tree, "", &[("TMPDIR", &tmp)]);

    let quiet = (Some(0), "", "");
    assert_eq!((ran.code, ran.out.as_str(), ran.err.as_str()), quiet);
}

// A committed repository that holds one Python class.
fn invoice_repo(tree: &Tree) -> PathBuf {
    let repo = tree.create_dir("repo").expect("the repository directory");
    tree.create_dir("repo/src/billing")
        .expect("a source directory");
    tree.write_file(
        "repo/src/billing/invoice.py",
        "class InvoiceTotal:\n    pass\n",
    )
    .expect("a source file");
    let vars = env(tree, &[]);
    for args in [
        &["init", "--quiet", "-b", "main"][..],
        &["add", "-A"],
        &["commit", "--quiet", "-m", "fixture"],
    ] {
        drop(io::testing::run_git(&repo, args, &vars, &IDENTITY));
    }
    repo
}

// The reasons of the gate log's lines, each with its hook.
fn logged(log: &Path) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(log).expect("the gate log");
    let lines = text.lines().map(|line| {
        let line: serde_json::Value = serde_json::from_str(line).expect("a JSON line");
        let field = |key: &str| line[key].as_str().unwrap_or("?").to_owned();
        (field("hook"), field("reason"))
    });
    lines.collect()
}

// The hooks of the trace surface come before the start-up directory test too.
#[test]
fn the_trace_hooks_on_an_empty_stdin_in_a_removed_directory_write_nothing() {
    let tree = sandbox();
    let log = tree.join("gate.jsonl");
    let shown = log.to_str().expect("a UTF-8 log path");
    let tmp = tree.path().display().to_string();
    let hooks = ["read-card", "ledger-mark", "ledger-reset"];
    for hook in hooks {
        tree.create_dir("gone").expect("the start directory");
        let mut command = Command::new("sh");
        command.args(["-c", IN_REMOVED_DIRECTORY, BINARY, hook]);

        let ran = fed(command, &tree, "", &[("TMPDIR", &tmp), (GATE_LOG, shown)]);

        let quiet = (Some(0), "", "");
        assert_eq!(
            (ran.code, ran.out.as_str(), ran.err.as_str()),
            quiet,
            "{hook}"
        );
    }
    let expected = hooks.map(|hook| (hook.to_owned(), "no_session_id".to_owned()));
    assert_eq!(logged(&log), expected);
}

#[test]
fn two_read_card_processes_make_one_ledger_claim() {
    let tree = sandbox();
    let repo = invoice_repo(&tree);
    let log = tree.join("gate.jsonl");
    let shown = log.to_str().expect("a UTF-8 log path");
    let tmp = tree.path().display().to_string();
    let payload = serde_json::json!({
        "session_id": "s1",
        "hook_event_name": "PostToolUse",
        "tool_name": "Read",
        "tool_input": {"file_path": repo.join("src/billing/invoice.py")},
    });
    let extra = [("TMPDIR", tmp.as_str()), (GATE_LOG, shown)];
    let children: Vec<Child> = (0..2)
        .map(|_| {
            let mut command = Command::new(BINARY);
            command.args(["hook", "read-card"]);
            started(command, &tree, &payload.to_string(), &extra)
        })
        .collect();

    let runs: Vec<Ran> = children.into_iter().map(finished).collect();

    for ran in &runs {
        assert_eq!((ran.code, ran.err.as_str()), (Some(0), ""));
    }
    let mut reasons: Vec<String> = logged(&log).into_iter().map(|(_, reason)| reason).collect();
    reasons.sort();
    assert_eq!(reasons.len(), 2, "{reasons:?}");
    assert_eq!(
        reasons.iter().filter(|reason| *reason == "held").count(),
        1,
        "{reasons:?}"
    );
    let ledger = tree.join("agentless_ledger");
    let sessions: Vec<PathBuf> = std::fs::read_dir(&ledger)
        .expect("the ledger")
        .map(|entry| entry.expect("a session").path())
        .collect();
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    let entries = std::fs::read_dir(sessions[0].join("main")).expect("the main context");
    assert_eq!(entries.count(), 1);
}

#[test]
fn hook_prompt_map_names_the_files_that_rank_first_for_a_first_prompt() {
    let tree = sandbox();
    let repo = invoice_repo(&tree);
    let payload = serde_json::json!({
        "session_id": "s1",
        "cwd": repo,
        "hook_event_name": "UserPromptSubmit",
        "prompt": "InvoiceTotal is wrong in `src/billing/invoice.py`.",
    });
    let tmp = tree.path().display().to_string();

    let ran = gate(
        &tree,
        "prompt-map",
        &payload.to_string(),
        &[("TMPDIR", &tmp)],
    );

    assert_eq!((ran.code, ran.err.as_str()), (Some(0), ""));
    let reply: serde_json::Value = serde_json::from_str(&ran.out).expect("one JSON object");
    let context = reply["hookSpecificOutput"]["additionalContext"].as_str();
    let head = "Repository map (agentless-mcp): for src/billing/invoice.py, InvoiceTotal in this \
                task, the files that rank first are src/billing/invoice.py";
    assert!(
        context.is_some_and(|text| text.starts_with(head)),
        "{}",
        ran.out
    );
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
