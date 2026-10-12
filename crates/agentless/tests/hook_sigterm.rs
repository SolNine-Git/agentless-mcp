//! The erosion hook under SIGTERM: a call still running logs "terminated" and exits 0, as the
//! Python hook does.
#![cfg(target_os = "linux")]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use io::testing::Tree;

const BINARY: &str = env!("CARGO_BIN_EXE_agentless-mcp");
const WAIT: Duration = Duration::from_secs(5);
const STEP: Duration = Duration::from_millis(10);
// SIGTERM is signal 15, so it is bit 14 of the caught-signal mask in /proc/<pid>/status.
const SIGTERM_BIT: u64 = 1 << 14;

struct Terminated {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

// The child reads the payload from a pipe that stays open, so it waits in the read.
fn hook_waiting_on_stdin(tree: &Tree, log: &Path) -> Child {
    let path = std::env::var_os("PATH").expect("the test finds kill through PATH");
    let home = tree.path().join("home");
    Command::new(BINARY)
        .args(["hook", "erosion"])
        .current_dir(tree.path())
        .env_clear()
        .env("PATH", path)
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("AGENTLESS_EROSION_LOG", log)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts")
}

// True once the process has a SIGTERM handler, read from its status file.
fn catches_sigterm(pid: u32) -> bool {
    let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
        return false;
    };
    status
        .lines()
        .find_map(|line| line.strip_prefix("SigCgt:"))
        .and_then(|mask| u64::from_str_radix(mask.trim(), 16).ok())
        .is_some_and(|mask| mask & SIGTERM_BIT != 0)
}

fn wait_until(mut ready: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < WAIT {
        if ready() {
            return true;
        }
        thread::sleep(STEP);
    }
    false
}

fn read_all(source: Option<impl Read>) -> String {
    let mut text = String::new();
    if let Some(mut source) = source {
        source.read_to_string(&mut text).expect("the pipe reads");
    }
    text
}

// Sends SIGTERM once the hook waits for its payload. A hook that does not exit in time is
// killed, and the test fails.
fn terminated(tree: &Tree, log: &Path) -> Terminated {
    let mut child = hook_waiting_on_stdin(tree, log);
    let stdin = child.stdin.take();
    assert!(
        wait_until(|| catches_sigterm(child.id())),
        "the hook never caught SIGTERM"
    );
    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(sent.success(), "kill: {sent}");
    let mut status = None;
    let exited = wait_until(|| {
        status = child.try_wait().expect("the wait works");
        status.is_some()
    });
    if !exited {
        child.kill().expect("the stuck child is killed");
        child.wait().expect("the killed child is reaped");
    }
    drop(stdin);
    assert!(exited, "the hook hung after SIGTERM");
    Terminated {
        status: status.expect("the hook exited"),
        stdout: read_all(child.stdout.take()),
        stderr: read_all(child.stderr.take()),
    }
}

#[test]
fn sigterm_while_the_payload_is_read_logs_terminated_and_exits_0() {
    let tree = Tree::new().expect("a temporary tree");
    let log = tree.path().join("log/erosion.jsonl");
    let done = terminated(&tree, &log);
    assert_eq!(done.status.code(), Some(0), "{}", done.status);
    assert_eq!(done.stdout, "");
    assert_eq!(done.stderr, "");
    let line = std::fs::read_to_string(&log).expect("the hook logged its record");
    assert_eq!(line.lines().count(), 1, "{line}");
    assert!(
        line.contains(r#""outcome": "degraded", "reason": "terminated""#),
        "{line}"
    );
    assert!(line.contains(r#""session_id": null"#), "{line}");
}

#[test]
fn sigterm_with_a_log_that_cannot_be_appended_exits_0_with_the_failure_line() {
    let tree = Tree::new().expect("a temporary tree");
    tree.write_file("blocker", "a file, not a directory")
        .expect("the blocking file");
    let log: PathBuf = tree.path().join("blocker/erosion.jsonl");
    let done = terminated(&tree, &log);
    assert_eq!(done.status.code(), Some(0), "{}", done.status);
    assert_eq!(done.stdout, "");
    let first = done
        .stderr
        .lines()
        .next()
        .unwrap_or("(no stderr)")
        .to_owned();
    let expected = format!(
        "erosion gate cannot append to {}: tool_use_id=None",
        log.display()
    );
    assert_eq!(first, expected, "{}", done.stderr);
}
