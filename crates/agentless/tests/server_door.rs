//! The agentless-mcp-server door: the thin binary runs its sibling under the server's name, which
//! parses the server's command line and serves one stdio session.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use io::testing::Tree;
use serde_json::{Value, json};

const DOOR: &str = env!("CARGO_BIN_EXE_agentless-mcp-server");
const BINARY: &str = env!("CARGO_BIN_EXE_agentless-mcp");
const DEADLINE: Duration = Duration::from_secs(30);
const MAPPING: &str = "| v1 tool (behind `--surface v1`) | v2 call |";

struct Ran {
    code: Option<i32>,
    out: String,
    err: String,
}

fn sandbox() -> Tree {
    let tree = Tree::new().unwrap_or_else(|error| panic!("a temporary tree: {error}"));
    tree.create_dir("home").expect("a home directory");
    tree.create_dir("repo").expect("a repository directory");
    tree
}

fn env(tree: &Tree) -> Vec<(OsString, OsString)> {
    let mut vars = io::testing::hermetic_env(&tree.path().join("home"));
    vars.push((
        OsString::from("AGENTLESS_MCP_NO_AUTO_INDEX"),
        OsString::from("1"),
    ));
    vars
}

fn run(program: &str, tree: &Tree, args: &[&str]) -> Ran {
    let spec = io::Spec {
        program: OsString::from(program),
        args: args.iter().map(OsString::from).collect(),
        cwd: tree.path().to_path_buf(),
        env: env(tree),
        timeout: DEADLINE,
        stdout_cap: 1 << 20,
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
        out: String::from_utf8_lossy(&stdout).into_owned(),
        err: String::from_utf8_lossy(&stderr).into_owned(),
    }
}

#[test]
fn the_door_prints_the_server_version() {
    let tree = sandbox();
    let ran = run(DOOR, &tree, &["--version"]);
    let want = format!("agentless-mcp-server {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(
        (ran.code, ran.out.as_str(), ran.err.as_str()),
        (Some(0), want.as_str(), "")
    );
}

#[test]
fn the_door_refuses_a_command_line_with_the_reference_text() {
    let tree = sandbox();
    let ran = run(DOOR, &tree, &["--transport", "http", "--port", "0"]);
    assert_eq!(ran.code, Some(2));
    assert_eq!(ran.out, "");
    let error = "agentless-mcp-server: error: --port 0 is outside 1-65535.";
    assert!(ran.err.contains(error), "{}", ran.err);
    let report = "agentless-mcp-server: received argv: ['--transport', 'http', '--port', '0']\n";
    assert!(ran.err.ends_with(report), "{}", ran.err);
}

// The refusal of a removed surface names the guide section, and that section answers.
#[test]
fn the_guide_section_of_the_surface_refusal_answers() {
    let tree = sandbox();
    let refused = run(DOOR, &tree, &["--surface", "v1"]);
    assert_eq!(refused.code, Some(2));
    assert!(
        refused
            .err
            .contains("agentless-mcp guide --section the-two-surfaces.")
    );
    let guide = run(BINARY, &tree, &["guide", "--section", "the-two-surfaces"]);
    assert_eq!(guide.code, Some(0), "{}", guide.err);
    assert!(guide.out.contains(MAPPING), "{}", guide.out);
}

fn request(id: u64, method: &str, params: &Value) -> String {
    format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    )
}

// Waits for the server's exit until the deadline, then kills it.
fn ended(child: &mut Child) -> Option<i32> {
    let start = Instant::now();
    while start.elapsed() < DEADLINE {
        if let Some(status) = child.try_wait().expect("a waitable child") {
            return status.code();
        }
        thread::sleep(Duration::from_millis(20));
    }
    drop(child.kill());
    panic!("the server did not end after its input closed");
}

// One stdio session through the thin binary: initialize, the tool list, one call, then the end of
// input ends the server with 0.
#[test]
fn the_door_serves_one_stdio_session() {
    let tree = sandbox();
    let mut child = Command::new(DOOR)
        .args(["--allow-client-roots", "--root", "repo"])
        .current_dir(tree.path())
        .env_clear()
        .envs(env(&tree))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("a started server");
    let stdout = child.stdout.take().expect("a stdout pipe");
    let (sender, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender.send(line.expect("a line")).is_err() {
                return;
            }
        }
    });
    let mut stdin = child.stdin.take().expect("a stdin pipe");
    let hello = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "door", "version": "0"},
    });
    let mut answer = |text: String| {
        stdin.write_all(text.as_bytes()).expect("a written request");
        let line = lines.recv_timeout(DEADLINE).expect("an answer");
        serde_json::from_str::<Value>(&line).expect("a JSON answer")
    };
    let initialized = answer(request(1, "initialize", &hello));
    assert_eq!(initialized["result"]["serverInfo"]["name"], "agentless-mcp");
    let notice = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    let listed = answer(format!(
        "{notice}\n{}",
        request(2, "tools/list", &json!({}))
    ));
    assert_eq!(listed["result"]["tools"].as_array().map(Vec::len), Some(6));
    let call = json!({"name": "capabilities", "arguments": {}});
    let called = answer(request(3, "tools/call", &call));
    assert_eq!(called["result"]["isError"], false, "{called}");
    drop(stdin);
    assert_eq!(ended(&mut child), Some(0));
}
