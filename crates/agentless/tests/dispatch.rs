//! The binary: what it sends to the command line and to the hook, its exit codes, and a closed
//! stdout.

use std::ffi::OsString;
use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use io::testing::Tree;

const BINARY: &str = env!("CARGO_BIN_EXE_agentless-mcp");
const DEADLINE: Duration = Duration::from_secs(30);
const HOOK_REFUSAL: &str = "agentless-mcp: hook takes one argument: erosion\n";

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
    let path = std::env::var_os("PATH").expect("the tests find git through PATH");
    let home = tree.path().join("home");
    let mut vars = vec![
        (OsString::from("PATH"), path),
        (OsString::from("HOME"), home.clone().into_os_string()),
        (
            OsString::from("XDG_CACHE_HOME"),
            home.join(".cache").into_os_string(),
        ),
        (OsString::from("GIT_CONFIG_NOSYSTEM"), OsString::from("1")),
        (
            OsString::from("GIT_CONFIG_GLOBAL"),
            OsString::from("/dev/null"),
        ),
    ];
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
fn a_hook_command_line_other_than_erosion_is_refused() {
    let tree = sandbox();
    for args in [
        &["hook"][..],
        &["hook", "nope"],
        &["hook", "erosion", "extra"],
    ] {
        let ran = run(&tree, args, &[]);
        assert_eq!(
            (ran.code, ran.out.as_str(), ran.err.as_str()),
            (Some(2), "", HOOK_REFUSAL)
        );
    }
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
