//! The agentless-mcp-server door: the thin binary runs its sibling under the server's name, which
//! parses the server's command line and serves one stdio session, with the hook note on its first
//! answer; and the command line's trace and search, which answer as the tools answer.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use io::testing::{Identity, Tree, run_git};
use serde_json::{Value, json};

const DOOR: &str = env!("CARGO_BIN_EXE_agentless-mcp-server");
const BINARY: &str = env!("CARGO_BIN_EXE_agentless-mcp");
const DEADLINE: Duration = Duration::from_secs(30);
const SURFACE_REFUSED: &str = "agentless-mcp-server: error: --surface is no longer served: this \
     release publishes trace and search only. Drop --surface.\n";
const ALL_MISSING: &str = "hooks: 7 of 7 agentless hook entries are missing";
// The commit of every rig, as the render goldens pin it.
const IDENTITY: Identity = Identity {
    name: "render goldens",
    email: "render@example.invalid",
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
    tree.create_dir("repo").expect("a repository directory");
    tree
}

// The settings folder is below the tree, so no test reads the user's own settings.
fn env(tree: &Tree) -> Vec<(OsString, OsString)> {
    let mut vars = io::testing::hermetic_env(&tree.path().join("home"));
    vars.push((
        OsString::from("AGENTLESS_MCP_NO_AUTO_INDEX"),
        OsString::from("1"),
    ));
    vars.push((
        OsString::from("CLAUDE_CONFIG_DIR"),
        tree.path().join("config").into_os_string(),
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

// The server publishes one surface, so --surface of any value is refused at start-up.
#[test]
fn the_door_refuses_every_surface() {
    let tree = sandbox();
    for surface in ["v1", "v2", "both", "trace"] {
        let refused = run(DOOR, &tree, &["--surface", surface]);
        assert_eq!(
            (refused.code, refused.out.as_str()),
            (Some(2), ""),
            "{surface}"
        );
        assert!(refused.err.contains(SURFACE_REFUSED), "{}", refused.err);
    }
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

// Waits for the server's exit until the deadline and sends a ping at each poll: after a failed
// write, the server and the reference end only when their blocked input read returns.
fn ended_while_pinged(child: &mut Child, stdin: &mut ChildStdin) -> Option<i32> {
    let start = Instant::now();
    let mut id = 2;
    while start.elapsed() < DEADLINE {
        if let Some(status) = child.try_wait().expect("a waitable child") {
            return status.code();
        }
        id += 1;
        let ping = json!({"jsonrpc": "2.0", "id": id, "method": "ping"});
        if let Err(error) = stdin.write_all(format!("{ping}\n").as_bytes()) {
            assert_eq!(error.kind(), ErrorKind::BrokenPipe, "{error}");
        }
        thread::sleep(Duration::from_millis(100));
    }
    drop(child.kill());
    panic!("the server did not end with its output closed");
}

// One stdio session of the door: the requests go in on stdin, and each answer is one line.
struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    id: u64,
}

impl Session {
    fn start(tree: &Tree, args: &[&str]) -> Session {
        let mut child = Command::new(DOOR)
            .args(args)
            .current_dir(tree.path())
            .env_clear()
            .envs(env(tree))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("a started server");
        let stdout = child.stdout.take().expect("a stdout pipe");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || forwarded(stdout, &sender));
        let stdin = child.stdin.take();
        let mut session = Session {
            child,
            stdin,
            lines,
            id: 0,
        };
        let hello = json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "door", "version": "0"},
        });
        let initialized = session.request("initialize", &hello);
        assert_eq!(initialized["result"]["serverInfo"]["name"], "agentless-mcp");
        let notice = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        session.write(&format!("{notice}\n"));
        session
    }

    fn write(&mut self, text: &str) {
        let stdin = self.stdin.as_mut().expect("an open input");
        stdin.write_all(text.as_bytes()).expect("a written request");
    }

    fn request(&mut self, method: &str, params: &Value) -> Value {
        self.id += 1;
        self.write(&request(self.id, method, params));
        let line = self.lines.recv_timeout(DEADLINE).expect("an answer");
        serde_json::from_str::<Value>(&line).expect("a JSON answer")
    }

    // The text of a tool call and its error flag.
    fn call(&mut self, tool: &str, arguments: &Value) -> (String, bool) {
        let called = self.request("tools/call", &json!({"name": tool, "arguments": arguments}));
        let text = called["result"]["content"][0]["text"].as_str();
        let error = called["result"]["isError"].as_bool();
        (
            text.expect("a text answer").to_owned(),
            error.expect("an error flag"),
        )
    }

    // The end of input ends the server, and the exit code says how.
    fn ended(mut self) -> Option<i32> {
        drop(self.stdin.take());
        ended(&mut self.child)
    }
}

// Sends each line of `stdout` until the receiver goes.
fn forwarded(stdout: impl Read, sender: &mpsc::Sender<String>) {
    for line in BufReader::new(stdout).lines() {
        if sender.send(line.expect("a line")).is_err() {
            return;
        }
    }
}

// One stdio session through the thin binary: initialize, the tool list, the first call with the
// hook note, then the end of input ends the server with 0.
#[test]
fn the_door_serves_one_stdio_session() {
    let tree = sandbox();
    let mut session = Session::start(&tree, &["--allow-client-roots", "--root", "repo"]);
    let listed = session.request("tools/list", &json!({}));
    let names: Vec<&str> = (listed["result"]["tools"].as_array().into_iter().flatten())
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, ["trace", "search"]);
    let (first, error) = session.call("search", &json!({"files": "*"}));
    assert!(!error && first.contains(ALL_MISSING), "{first}");
    let (second, _) = session.call("search", &json!({"files": "*"}));
    assert!(!second.contains("hooks: "), "{second}");
    assert_eq!(session.ended(), Some(0));
}

// The entries that `hook install` writes for this binary are current, so the door gives no note.
#[test]
fn a_door_with_current_hook_entries_gives_no_note() {
    let tree = sandbox();
    tree.create_dir("config").expect("a settings folder");
    let binary = io::resolve(Path::new(BINARY)).expect("the binary's path");
    let mut events: Vec<(&str, Vec<Value>)> = Vec::new();
    for entry in app::hooks::entries(app::hooks::DEFAULT_SERVER_NAME) {
        let command = app::hooks::command(&binary.display().to_string(), entry.hook);
        let handler = json!({"type": "command", "command": command, "timeout": entry.timeout});
        let mut group = json!({"hooks": [handler]});
        if let Some(matcher) = &entry.matcher {
            group["matcher"] = json!(matcher);
        }
        match events
            .iter_mut()
            .find(|(event, _)| *event == entry.event.as_str())
        {
            Some((_, groups)) => groups.push(group),
            None => events.push((entry.event.as_str(), vec![group])),
        }
    }
    let hooks: serde_json::Map<String, Value> = (events.into_iter())
        .map(|(event, groups)| (event.to_owned(), Value::Array(groups)))
        .collect();
    let settings = json!({"hooks": hooks}).to_string();
    tree.write_file("config/settings.json", settings)
        .expect("a settings file");
    let mut session = Session::start(&tree, &["--root", "repo"]);
    let (first, error) = session.call("search", &json!({"files": "*"}));
    assert!(!error && !first.contains("hooks"), "{first}");
    assert_eq!(session.ended(), Some(0));
}

// The client closes its end of the server's output after the initialize answer and keeps the input
// open: the next answer's write fails, and the door exits 120 with one stderr line.
#[test]
fn a_closed_output_ends_the_door_with_120_and_one_line() {
    let tree = sandbox();
    let mut child = Command::new(DOOR)
        .args(["--root", "repo"])
        .current_dir(tree.path())
        .env_clear()
        .envs(env(&tree))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("a started server");
    let stdout = child.stdout.take().expect("a stdout pipe");
    let (sender, first) = mpsc::channel();
    // The reader drops the pipe before it hands over the line.
    thread::spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout).read_line(&mut line);
        drop(sender.send(read.map(|_| line)));
    });
    let mut stdin = child.stdin.take().expect("a stdin pipe");
    let hello = json!({
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "door", "version": "0"},
    });
    let initialize = request(1, "initialize", &hello);
    stdin
        .write_all(initialize.as_bytes())
        .expect("a written request");
    let answer = first.recv_timeout(DEADLINE).expect("an answer");
    assert!(answer.expect("a read line").contains(r#""id":1"#));
    let notice = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    stdin
        .write_all(format!("{notice}\n").as_bytes())
        .expect("a written notice");
    assert_eq!(ended_while_pinged(&mut child, &mut stdin), Some(120));
    let mut err = String::new();
    let mut stderr = child.stderr.take().expect("a stderr pipe");
    stderr.read_to_string(&mut err).expect("the stderr text");
    let line = "agentless-mcp-server: cannot write to standard output: BrokenPipeError: [Errno 32] \
                Broken pipe\n";
    assert_eq!(err, line);
    drop(stdin);
}

fn characterization(parts: &[&str]) -> PathBuf {
    let base = [env!("CARGO_MANIFEST_DIR"), "../../tests/characterization"];
    base.iter().chain(parts).collect()
}

// Each request beside a trace or search golden, by the golden's stem, in name order.
fn requests() -> Vec<(String, Value)> {
    let mut found = Vec::new();
    for kind in ["trace", "search"] {
        let dir = characterization(&["goldens", kind]);
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|error| panic!("{error}"));
        for entry in entries {
            let path = entry.unwrap_or_else(|error| panic!("{error}")).path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if let Some(stem) = name.strip_suffix(".request.json") {
                let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{error}"));
                let request = serde_json::from_str(&text).unwrap_or_else(|error| panic!("{error}"));
                found.push((format!("{kind}/{stem}"), request));
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

// A copy of the request's fixture repository with its extra files, committed with the pinned
// identity, then the request's history, as the render goldens build it.
fn rig(tree: &Tree, request: &Value) -> PathBuf {
    let fixture = request["fixture"].as_str().expect("a fixture name");
    copied(&characterization(&["fixtures", fixture]), tree, "repo");
    written(&request["files"], tree);
    for (path, size) in request["sparse"].as_object().into_iter().flatten() {
        let relative = format!("repo/{path}");
        tree.write_file(&relative, "").expect("a sparse file");
        let size = size.as_u64().expect("a size");
        tree.set_len(&relative, size).expect("a length");
    }
    let root = tree.path().join("repo");
    let env = env(tree);
    for args in [
        &["init", "--quiet", "-b", "main"][..],
        &["add", "-A"],
        &["commit", "--quiet", "-m", "fixture"],
    ] {
        drop(run_git(&root, args, &env, &IDENTITY));
    }
    let git = |args: &[&str]| drop(run_git(&root, args, &env, &IDENTITY));
    if let Some(tag) = request["tag"].as_str() {
        git(&["tag", tag]);
    }
    for commit in request["commits"].as_array().into_iter().flatten() {
        written(&commit["files"], tree);
        deleted(&commit["deleted"], tree);
        let message = commit["message"].as_str().expect("a commit message");
        git(&["add", "-A"]);
        git(&["commit", "--quiet", "-m", message]);
    }
    written(&request["working"], tree);
    deleted(&request["deleted"], tree);
    root
}

fn deleted(paths: &Value, tree: &Tree) {
    for path in paths.as_array().into_iter().flatten() {
        let relative = format!("repo/{}", path.as_str().expect("a path"));
        let removed = io::remove_file(&tree.join(&relative)).expect("a remove");
        assert!(removed, "{relative}: no file to delete");
    }
}

fn written(files: &Value, tree: &Tree) {
    for (path, text) in files.as_object().into_iter().flatten() {
        let relative = format!("repo/{path}");
        if let Some((parent, _)) = relative.rsplit_once('/') {
            tree.create_dir(parent).expect("a folder");
        }
        let text = text.as_str().expect("a file's text");
        tree.write_file(&relative, text).expect("an extra file");
    }
}

fn copied(from: &Path, tree: &Tree, below: &str) {
    tree.create_dir(below).expect("a folder");
    let entries = std::fs::read_dir(from).unwrap_or_else(|error| panic!("{error}"));
    for entry in entries {
        let path = entry.unwrap_or_else(|error| panic!("{error}")).path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a name");
        let relative = format!("{below}/{name}");
        if path.is_dir() {
            copied(&path, tree, &relative);
        } else {
            tree.copy_file(&path, &relative).expect("a copied file");
        }
    }
}

// The command line that asks what `arguments` ask the tool `tool`.
fn command_line(tool: &str, arguments: &Value, root: &Path) -> Vec<String> {
    let mut args = vec![tool.to_owned()];
    let text = |name: &str| arguments[name].as_str().map(str::to_owned);
    let flags = [
        ("more", "--more"),
        ("rev", "--rev"),
        ("files", "--files"),
        ("path", "--path"),
    ];
    for (name, flag) in flags {
        if let Some(value) = text(name) {
            args.extend([flag.to_owned(), value]);
        }
    }
    for (name, flag) in [("context", "--context"), ("limit", "--limit")] {
        if let Some(count) = arguments[name].as_u64() {
            args.extend([flag.to_owned(), count.to_string()]);
        }
    }
    if arguments["ignore_case"] == true {
        args.push("--ignore-case".to_owned());
    }
    args.extend([
        "--repo".to_owned(),
        root.display().to_string(),
        "--".to_owned(),
    ]);
    match &arguments["target"] {
        Value::String(target) => args.push(target.clone()),
        Value::Array(targets) => args
            .extend((targets.iter()).map(|target| target.as_str().expect("a target").to_owned())),
        _ => args.extend(text("pattern")),
    }
    args
}

// The answer below its receipt, whose root and cache fields differ between the two doors.
fn body(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .skip_while(|line| line.starts_with("// "))
        .collect();
    lines.join("\n")
}

// What the command line gives for one request: the answer body, or the refusal of a call that
// the tool refuses.
fn command_answer(tree: &Tree, args: &[String]) -> (String, bool) {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let ran = run(BINARY, tree, &args);
    if ran.code == Some(0) {
        return (body(&ran.out), false);
    }
    let refusal = ran.err.lines().last().unwrap_or_default();
    let refusal = refusal.strip_prefix("agentless-mcp: ").unwrap_or(refusal);
    (refusal.to_owned(), true)
}

fn tool_answer(session: &mut Session, tool: &str, arguments: &Value) -> (String, bool) {
    let (text, error) = session.call(tool, arguments);
    if error {
        let prefix = format!("Error calling tool '{tool}': ");
        (text.strip_prefix(&prefix).unwrap_or(&text).to_owned(), true)
    } else {
        (body(&text), false)
    }
}

// A late git gives a card that says so in one door and not the other; a retry reads the commit.
const LAST_CHANGE_TRIES: usize = 3;

// nextest runs the parts side by side, so each part stays inside the 60 s test limit.
const PARTS: usize = 4;

// For every request beside a trace or search golden, the command line answers with the text of
// the tool, below the receipt, or refuses with the refusal of the tool.
fn doors_answer_alike(part: usize) {
    let mut differences = Vec::new();
    let requests = requests();
    assert!(requests.len() > 50, "{} requests", requests.len());
    for (stem, request) in requests.iter().skip(part).step_by(PARTS) {
        let tree = sandbox();
        let root = rig(&tree, request);
        let mut session = Session::start(&tree, &["--root", &root.display().to_string()]);
        let tool = request["tool"].as_str().expect("a tool");
        let args = command_line(tool, &request["arguments"], &root);
        let mut answers = Vec::new();
        for _ in 0..LAST_CHANGE_TRIES {
            let answer = tool_answer(&mut session, tool, &request["arguments"]);
            let command = command_answer(&tree, &args);
            let late = [&answer.0, &command.0]
                .iter()
                .any(|text| text.contains("not read in"));
            answers = vec![answer, command];
            if answers[0] == answers[1] || !late {
                break;
            }
        }
        assert!(!answers[0].0.is_empty(), "{stem}: an empty answer");
        if answers[0] != answers[1] {
            differences.push(format!(
                "{stem}: {args:?}\ntool {:?}\n cli {:?}",
                answers[0], answers[1]
            ));
        }
        assert_eq!(session.ended(), Some(0), "{stem}");
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

#[test]
fn the_command_line_answers_as_the_tools_answer_part_1() {
    doors_answer_alike(0);
}

#[test]
fn the_command_line_answers_as_the_tools_answer_part_2() {
    doors_answer_alike(1);
}

#[test]
fn the_command_line_answers_as_the_tools_answer_part_3() {
    doors_answer_alike(2);
}

#[test]
fn the_command_line_answers_as_the_tools_answer_part_4() {
    doors_answer_alike(3);
}
