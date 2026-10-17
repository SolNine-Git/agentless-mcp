//! SIGINT through the binary: a validate run that SIGINT stops kills its test command, releases its
//! worktree, prints the call's error, and then ends by SIGINT.

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use io::testing::{Identity, Tree, hermetic_env, run_git};

const BINARY: &str = env!("CARGO_BIN_EXE_agentless-mcp");
const WAIT: Duration = Duration::from_secs(30);
const STEP: Duration = Duration::from_millis(10);
const SIGINT: i32 = 2;
const IDENTITY: Identity = Identity {
    name: "agentless-mcp tests",
    email: "tests@example.invalid",
    date: "2026-01-01T00:00:00Z",
};
const CANDIDATE: &str =
    r#"{"edits": [{"path": "app.py", "search": "    return 1\n", "replace": "    return 2\n"}]}"#;

// A committed repository with app.py, and one candidate that edits it.
fn repository(tree: &Tree) -> PathBuf {
    let root = tree.create_dir("repo").expect("the repository directory");
    tree.write_file("repo/app.py", "def f():\n    return 1\n")
        .expect("app.py is written");
    tree.create_dir("cands").expect("the candidates directory");
    tree.write_file("cands/a.json", CANDIDATE)
        .expect("the candidate is written");
    let env = hermetic_env(&tree.join("home"));
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "-A"],
        &["commit", "-q", "-m", "init"],
    ] {
        let _printed = run_git(&root, args, &env, &IDENTITY);
    }
    root
}

// A validate run whose test command marks its start and then outlives every wait of the test.
fn validate(tree: &Tree, root: &Path, started: &Path) -> Child {
    let test_cmd = format!("sh -c 'touch {}; exec sleep 600'", started.display());
    let candidates = tree.join("cands");
    Command::new(BINARY)
        .arg("validate")
        .arg("--candidates")
        .arg(&candidates)
        .args(["--test-cmd", &test_cmd])
        .current_dir(root)
        .env_clear()
        .envs(hermetic_env(&tree.join("home")))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary starts")
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

// Waits for the exit; a run that does not end in time is killed, and the test fails.
fn ended(child: &mut Child) -> ExitStatus {
    let mut status = None;
    let exited = wait_until(|| {
        status = child.try_wait().expect("the wait works");
        status.is_some()
    });
    if !exited {
        child.kill().expect("the stuck run is killed");
        child.wait().expect("the killed run is reaped");
    }
    status.expect("the run ended after SIGINT")
}

fn read_all(source: Option<impl Read>) -> String {
    let mut text = String::new();
    if let Some(mut source) = source {
        source.read_to_string(&mut text).expect("the pipe reads");
    }
    text
}

#[test]
fn a_sigint_during_a_test_run_releases_the_worktree_prints_the_error_and_ends_by_sigint() {
    let tree = Tree::new().expect("a temporary tree");
    tree.create_dir("home").expect("a home directory");
    let root = repository(&tree);
    let started = tree.join("started");
    let mut child = validate(&tree, &root, &started);
    assert!(
        wait_until(|| started.exists()),
        "the test command never started"
    );
    let sent = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill runs");
    assert!(sent.success(), "kill: {sent}");
    let status = ended(&mut child);
    let stdout = read_all(child.stdout.take());
    let stderr = read_all(child.stderr.take());
    assert_eq!(status.signal(), Some(SIGINT), "{status} {stderr}");
    assert_eq!(stdout, "");
    assert_eq!(
        stderr,
        "agentless-mcp: validate was interrupted by SIGINT; no verdicts were written\n"
    );
    let env = hermetic_env(&tree.join("home"));
    let listed = run_git(&root, &["worktree", "list"], &env, &IDENTITY);
    let listed = String::from_utf8_lossy(&listed);
    assert_eq!(listed.lines().count(), 1, "{listed}");
    let scratch = tree.join("home/.cache/agentless-mcp/worktrees");
    let left = std::fs::read_dir(&scratch).map_or(0, Iterator::count);
    assert_eq!(
        left,
        0,
        "a scratch worktree is still in {}",
        scratch.display()
    );
}
