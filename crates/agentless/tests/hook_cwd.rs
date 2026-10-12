//! The erosion hook started in a deleted directory fails open, as the Python hook does. The
//! expected values are the Python hook's, measured with the same payloads.
#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use io::testing::Tree;

const BINARY: &str = env!("CARGO_BIN_EXE_agentless-mcp");
const WAIT: Duration = Duration::from_secs(30);
const STEP: Duration = Duration::from_millis(10);
// The shell enters `gone`, removes it, and then becomes the hook, so the hook starts there.
const IN_DELETED_DIRECTORY: &str = r#"cd gone && rmdir "$PWD" && exec "$0" hook erosion"#;

struct Ran {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    record: String,
}

fn env(tree: &Tree) -> Vec<(OsString, OsString)> {
    let path = std::env::var_os("PATH").expect("the test finds git and sh through PATH");
    let home = tree.path().join("home");
    let fixed = [
        ("PATH", path),
        ("HOME", home.clone().into_os_string()),
        ("XDG_CACHE_HOME", home.join(".cache").into_os_string()),
        ("GIT_CONFIG_NOSYSTEM", OsString::from("1")),
        ("GIT_CONFIG_GLOBAL", OsString::from("/dev/null")),
        ("GIT_AUTHOR_NAME", OsString::from("hook tests")),
        ("GIT_AUTHOR_EMAIL", OsString::from("hook@example.invalid")),
        ("GIT_COMMITTER_NAME", OsString::from("hook tests")),
        (
            "GIT_COMMITTER_EMAIL",
            OsString::from("hook@example.invalid"),
        ),
        (
            "AGENTLESS_EROSION_LOG",
            tree.path().join("log/erosion.jsonl").into_os_string(),
        ),
    ];
    fixed
        .into_iter()
        .map(|(name, value)| (OsString::from(name), value))
        .collect()
}

fn git(tree: &Tree, args: &[&str]) {
    let mut command = vec![OsString::from("-C"), tree.join("repo").into_os_string()];
    command.extend(args.iter().map(OsString::from));
    let spec = io::Spec {
        program: OsString::from("git"),
        args: command,
        cwd: PathBuf::from("."),
        env: env(tree),
        timeout: WAIT,
        stdout_cap: 1 << 16,
        stderr_cap: 1 << 16,
    };
    match io::run(&spec) {
        io::Outcome::Exited { status, .. } if status.success() => {}
        outcome => panic!("git {args:?}: {outcome:?}"),
    }
}

// A repository with one committed file, and the directory the hook starts in.
fn sandbox() -> Tree {
    let tree = Tree::new().expect("a temporary tree");
    tree.create_dir("repo").expect("the repository directory");
    tree.create_dir("gone").expect("the start directory");
    git(&tree, &["init", "--quiet", "-b", "main"]);
    tree.write_file("repo/a.py", "x = 1\n")
        .expect("a source file");
    git(&tree, &["add", "-A"]);
    git(&tree, &["commit", "--quiet", "-m", "fixture"]);
    tree
}

fn wait(child: &mut std::process::Child) -> ExitStatus {
    let started = Instant::now();
    while started.elapsed() < WAIT {
        if let Some(status) = child.try_wait().expect("the wait works") {
            return status;
        }
        thread::sleep(STEP);
    }
    child.kill().expect("the stuck child is killed");
    panic!("the hook did not exit within {WAIT:?}");
}

fn run_in_deleted_directory(tree: &Tree, payload: &str) -> Ran {
    let mut child = Command::new("sh")
        .args(["-c", IN_DELETED_DIRECTORY, BINARY])
        .current_dir(tree.path())
        .env_clear()
        .envs(env(tree))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the shell starts");
    let mut stdin = child.stdin.take().expect("a stdin pipe");
    stdin
        .write_all(payload.as_bytes())
        .expect("the payload is written");
    drop(stdin);
    let status = wait(&mut child);
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut out = child.stdout.take().expect("a stdout pipe");
    out.read_to_string(&mut stdout).expect("stdout reads");
    let mut err = child.stderr.take().expect("a stderr pipe");
    err.read_to_string(&mut stderr).expect("stderr reads");
    let record = std::fs::read_to_string(tree.path().join("log/erosion.jsonl"))
        .expect("the hook logged its record");
    Ran {
        status,
        stdout,
        stderr,
        record,
    }
}

fn write_payload(file_path: &Path, cwd: Option<&str>) -> String {
    let cwd = cwd.map_or(String::new(), |cwd| format!(r#""cwd": "{cwd}", "#));
    format!(
        r#"{{"session_id": "s1", "transcript_path": "/t", "tool_use_id": "u1", {cwd}"tool_name": "Write", "tool_input": {{"file_path": "{}", "content": "y = 2\n"}}}}"#,
        file_path.display()
    )
}

#[test]
fn an_absolute_file_path_is_gated_as_usual() {
    let tree = sandbox();
    let target = tree.join("repo/a.py");
    let ran = run_in_deleted_directory(&tree, &write_payload(&target, None));
    assert_eq!(ran.status.code(), Some(0), "{}", ran.stderr);
    assert_eq!(ran.stdout, "");
    assert_eq!(ran.stderr, "");
    assert!(
        ran.record.contains(r#""outcome": "clean", "reason": null"#),
        "{}",
        ran.record
    );
    let path = format!(r#""path": "{}""#, target.display());
    assert!(ran.record.contains(&path), "{}", ran.record);
}

#[test]
fn a_relative_file_path_without_a_payload_cwd_is_malformed() {
    let tree = sandbox();
    let ran = run_in_deleted_directory(&tree, &write_payload(Path::new("a.py"), None));
    assert_eq!(ran.status.code(), Some(0), "{}", ran.stderr);
    assert_eq!(ran.stdout, "");
    assert_eq!(ran.stderr, "");
    let expected =
        r#""outcome": "degraded", "reason": "malformed payload: payload.cwd is missing""#;
    assert!(ran.record.contains(expected), "{}", ran.record);
}

#[test]
fn a_relative_payload_cwd_needs_the_deleted_directory_and_is_an_internal_error() {
    let tree = sandbox();
    let ran = run_in_deleted_directory(&tree, &write_payload(Path::new("a.py"), Some("repo")));
    assert_eq!(ran.status.code(), Some(0), "{}", ran.stderr);
    assert_eq!(ran.stdout, "");
    let first = ran
        .stderr
        .lines()
        .next()
        .unwrap_or("(no stderr)")
        .to_owned();
    assert_eq!(
        first,
        "erosion gate internal error: tool_use_id=u1 path=None"
    );
    let expected = concat!(
        r#""outcome": "degraded", "reason": "internal error: FileNotFoundError: "#,
        r#"[Errno 2] No such file or directory""#
    );
    assert!(ran.record.contains(expected), "{}", ran.record);
}
