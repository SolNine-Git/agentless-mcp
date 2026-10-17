use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app::Session;
use rmcp::ServiceExt;
use serde_json::{Value as Json, json};
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf, WriteHalf,
};

use super::calls::{built, masked, reference, run_named};
use super::{Kept, Sandbox, quiet_ports};
use crate::dispatch::UNPLANNED;
use crate::offload::{Job, Offload};
use crate::{Arming, Ports, Server, ServerConfig};

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const READ_LIMIT: Duration = Duration::from_secs(20);
// How long a test waits to see that a call does not start while the gate is full.
const QUIET: Duration = Duration::from_millis(200);

// A client end of an in-process transport: raw JSON-RPC lines, no listening port.
struct Wire {
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    writer: WriteHalf<DuplexStream>,
}

impl Wire {
    fn start(server: Server) -> Wire {
        let (client, service) = tokio::io::duplex(1 << 22);
        tokio::spawn(async move {
            if let Ok(running) = server.serve(service).await {
                drop(running.waiting().await);
            }
        });
        let (reader, writer) = tokio::io::split(client);
        Wire {
            lines: BufReader::new(reader).lines(),
            writer,
        }
    }

    async fn send(&mut self, message: &Json) {
        let line = format!("{message}\n");
        self.writer
            .write_all(line.as_bytes())
            .await
            .expect("a sent line");
    }

    async fn line(&mut self) -> String {
        let read = tokio::time::timeout(READ_LIMIT, self.lines.next_line()).await;
        read.expect("a line before the limit")
            .expect("a readable line")
            .expect("an open stream")
    }

    async fn request(&mut self, id: u64, method: &str, params: Json) -> String {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        self.line().await
    }

    // The initialize answer for `version`, after which the session is ready.
    async fn initialized(&mut self, version: &str) -> String {
        let params = json!({"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "probe", "version": "0"}});
        let answer = self.request(1, "initialize", params).await;
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        answer
    }
}

fn parsed(line: &str) -> Json {
    serde_json::from_str(line).expect("a JSON line")
}

// One root, the probe repository, and no launch directory.
fn config_of(sandbox: &Sandbox, concurrency: usize) -> ServerConfig {
    ServerConfig {
        roots: vec![io::resolve(&sandbox.root.join("repo")).expect("a root")],
        roots_files: Vec::new(),
        launch_dir: None,
        max_concurrency: concurrency,
        call_limit: super::CALL_LIMIT,
        recache_grace: super::RECACHE_GRACE,
    }
}

fn server_of(sandbox: &Sandbox, concurrency: usize, ports: Ports) -> Server {
    Server::new(config_of(sandbox, concurrency), sandbox.services(), ports)
}

fn probe() -> (Json, Sandbox) {
    let run = run_named("probe");
    let sandbox = built(&run);
    (run, sandbox)
}

// The tools/list answer is the reference surface, key order and number forms included, and its
// line is the compact form of that surface byte for byte.
#[tokio::test(flavor = "multi_thread")]
async fn tools_list_is_the_reference_surface() {
    let (_, sandbox) = probe();
    let mut wire = Wire::start(server_of(&sandbox, 8, quiet_ports()));
    wire.initialized("2025-06-18").await;
    let line = wire.request(2, "tools/list", json!({})).await;
    let path = Path::new(MANIFEST_DIR).join("../app/fixtures/surface/surface.json");
    let surface: Json =
        serde_json::from_str(&fs::read_to_string(path).expect("the surface fixture"))
            .expect("a JSON fixture");
    let want = json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": surface["tools"]}});
    assert_eq!(line, want.to_string());
}

// The capabilities and the server name are the reference's; the version is this build's, and the
// protocol version echoes each one the reference knows, else falls back to the newest.
#[tokio::test(flavor = "multi_thread")]
async fn initialize_answers_with_the_reference_capabilities() {
    let (_, sandbox) = probe();
    let capabilities = json!({"experimental": {}, "logging": {}, "prompts": {"listChanged": false}, "resources": {"subscribe": false, "listChanged": false}, "tools": {"listChanged": true}, "extensions": {"io.modelcontextprotocol/ui": {}}});
    for (asked, answered) in [
        ("2024-11-05", "2024-11-05"),
        ("2025-03-26", "2025-03-26"),
        ("2025-06-18", "2025-06-18"),
        ("2025-11-25", "2025-11-25"),
        ("2026-07-28", "2025-11-25"),
        ("1999-01-01", "2025-11-25"),
    ] {
        let mut wire = Wire::start(server_of(&sandbox, 8, quiet_ports()));
        let answer = parsed(&wire.initialized(asked).await);
        let want = json!({"protocolVersion": answered, "capabilities": capabilities, "serverInfo": {"name": "agentless-mcp", "version": env!("CARGO_PKG_VERSION")}});
        assert_eq!(answer["result"], want, "{asked}");
    }
}

// Each line equals the reference answer of the stdio probe, the id aside.
#[tokio::test(flavor = "multi_thread")]
async fn the_protocol_answers_are_the_reference_answers() {
    let (_, sandbox) = probe();
    let mut wire = Wire::start(server_of(&sandbox, 8, quiet_ports()));
    wire.initialized("2025-06-18").await;
    let unknown = r#"{"jsonrpc":"2.0","id":7,"error":{"code":-32602,"message":"Invalid request parameters","data":""}}"#;
    for (id, method, want) in [
        (2, "ping", r#"{"jsonrpc":"2.0","id":2,"result":{}}"#),
        (
            3,
            "resources/list",
            r#"{"jsonrpc":"2.0","id":3,"result":{"resources":[]}}"#,
        ),
        (
            4,
            "resources/templates/list",
            r#"{"jsonrpc":"2.0","id":4,"result":{"resourceTemplates":[]}}"#,
        ),
        (
            5,
            "prompts/list",
            r#"{"jsonrpc":"2.0","id":5,"result":{"prompts":[]}}"#,
        ),
        (7, "bogus/method", unknown),
    ] {
        assert_eq!(wire.request(id, method, json!({})).await, want, "{method}");
    }
    let invalid = |id: u64| unknown.replace(":7,", &format!(":{id},"));
    let not_found = |id: u64| {
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}})
            .to_string()
    };
    let completion = json!({"ref": {"type": "ref/prompt", "name": "x"}, "argument": {"name": "a", "value": "b"}});
    for (id, method, params, want) in [
        (8, "tools/call", json!({"name": "orient", "arguments": [1]}), invalid(8)),
        (9, "tools/call", json!({"arguments": {}}), invalid(9)),
        (10, "completion/complete", completion, not_found(10)),
        (11, "resources/read", json!({"uri": "file:///x"}), r#"{"jsonrpc":"2.0","id":11,"error":{"code":-32002,"message":"Resource not found: Unknown resource: 'file:///x'"}}"#.to_owned()),
        (12, "prompts/get", json!({"name": "x"}), r#"{"jsonrpc":"2.0","id":12,"error":{"code":0,"message":"Unknown prompt: 'x'"}}"#.to_owned()),
        (13, "resources/subscribe", json!({"uri": "file:///x"}), not_found(13)),
        (14, "resources/unsubscribe", json!({"uri": "file:///x"}), not_found(14)),
    ] {
        assert_eq!(wire.request(id, method, params).await, want, "{method}");
    }
}

// The recorded deviations D2 and D3: rmcp answers these methods before any handler runs, or the
// handler would have to name a deprecated rmcp type (logging/setLevel).
#[tokio::test(flavor = "multi_thread")]
async fn the_deviating_protocol_answers_are_the_recorded_ones() {
    let (_, sandbox) = probe();
    let mut wire = Wire::start(server_of(&sandbox, 8, quiet_ports()));
    wire.initialized("2025-06-18").await;
    let discover = "request _meta is missing or has malformed required fields: \
                    io.modelcontextprotocol/protocolVersion, io.modelcontextprotocol/clientCapabilities";
    let calls = [
        ("server/discover", json!({}), -32602, discover),
        (
            "subscriptions/listen",
            json!({"notifications": {}}),
            -32601,
            "subscriptions/listen",
        ),
        ("tasks/get", json!({"taskId": "x"}), -32601, "tasks/get"),
        (
            "logging/setLevel",
            json!({"level": "debug"}),
            -32601,
            "logging/setLevel",
        ),
    ];
    for (id, (method, params, code, message)) in (10..).zip(calls) {
        let want = json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}});
        assert_eq!(
            wire.request(id, method, params).await,
            want.to_string(),
            "{method}"
        );
    }
}

// A tools/call answer is one text block with its error flag, in the reference key order.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_answer_is_one_text_block_with_its_error_flag() {
    let (run, sandbox) = probe();
    let mut wire = Wire::start(server_of(&sandbox, 8, quiet_ports()));
    wire.initialized("2025-06-18").await;
    let call = json!({"name": "read", "arguments": {"operation": "dir"}});
    let line = wire.request(2, "tools/call", call).await;
    let text = &parsed(&line)["result"]["content"][0]["text"];
    let normalised = sandbox.normalised(text.as_str().expect("a text"));
    let want = reference(&run, "read", &json!({"operation": "dir"}));
    assert_eq!(masked(&normalised), want);
    let shape = json!({"jsonrpc": "2.0", "id": 2, "result": {"content": [{"type": "text", "text": text}], "isError": false}});
    assert_eq!(line, shape.to_string());
    let refused = wire
        .request(3, "tools/call", json!({"name": "orient", "arguments": {}}))
        .await;
    assert_eq!(parsed(&refused)["result"]["isError"], true);
}

// The background index of the gate tests: each call reports that it started, then waits for the
// test to let it go on.
struct Held {
    started: Mutex<mpsc::Sender<()>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl Arming for Held {
    fn arm(&self, _session: &Session) {
        self.started
            .lock()
            .expect("a sender")
            .send(())
            .expect("a test that waits");
        let release = self.release.lock().expect("a receiver");
        release.recv_timeout(READ_LIMIT).expect("a release");
    }
}

fn held() -> (Arc<Held>, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (started, on_start) = mpsc::channel();
    let (release, on_release) = mpsc::channel();
    let arming = Held {
        started: Mutex::new(started),
        release: Mutex::new(on_release),
    };
    (Arc::new(arming), on_start, release)
}

fn held_ports(arming: Arc<Held>) -> Ports {
    Ports {
        arming,
        log: Arc::new(Kept::default()),
    }
}

fn find(id: u64) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "symbols", "arguments": {"operation": "find", "name": "alpha"}}})
}

// With one permit, a second call starts only after the first ends; with two, both work at once.
#[tokio::test(flavor = "multi_thread")]
async fn the_gate_holds_calls_beyond_its_permits() {
    for (permits, second_starts_early) in [(1, false), (2, true)] {
        let (_, sandbox) = probe();
        let (arming, started, release) = held();
        let mut wire = Wire::start(server_of(&sandbox, permits, held_ports(arming)));
        wire.initialized("2025-06-18").await;
        wire.send(&find(2)).await;
        started
            .recv_timeout(READ_LIMIT)
            .expect("the first call starts");
        wire.send(&find(3)).await;
        assert_eq!(
            started.recv_timeout(QUIET).is_ok(),
            second_starts_early,
            "{permits}"
        );
        for _ in 0..2 {
            release.send(()).expect("a held call");
        }
        let answers = [wire.line().await, wire.line().await];
        assert!(
            answers
                .iter()
                .all(|line| parsed(line)["result"]["isError"] == false)
        );
    }
}

// A cancelled call sends no answer, and its permit frees when its thread reaches a checkpoint.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_call_frees_its_permit_at_its_next_checkpoint() {
    let (_, sandbox) = probe();
    let (arming, started, release) = held();
    let mut wire = Wire::start(server_of(&sandbox, 1, held_ports(arming)));
    wire.initialized("2025-06-18").await;
    wire.send(&find(2)).await;
    started
        .recv_timeout(READ_LIMIT)
        .expect("the first call starts");
    let cancel = json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 2, "reason": "test"}});
    wire.send(&cancel).await;
    wire.send(&find(3)).await;
    release.send(()).expect("a held call");
    started
        .recv_timeout(READ_LIMIT)
        .expect("the second call starts");
    release.send(()).expect("a held call");
    let answer = parsed(&wire.line().await);
    assert_eq!(answer["id"], 3);
    assert_eq!(answer["result"]["isError"], false);
}

struct Panicking;

impl Arming for Panicking {
    fn arm(&self, _session: &Session) {
        panic!("an injected panic on the call's thread");
    }
}

struct Refusing;

impl Offload for Refusing {
    fn start(&self, _job: Job) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(11))
    }
}

// A panic on the call's thread, and a thread that the system refuses to start, each answer with
// the unplanned-error text; the server keeps answering.
#[tokio::test(flavor = "multi_thread")]
async fn a_panicking_or_refused_thread_answers_as_an_unplanned_failure() {
    let (_, sandbox) = probe();
    let ports = Ports {
        arming: Arc::new(Panicking),
        log: Arc::new(Kept::default()),
    };
    let panicking = server_of(&sandbox, 1, ports);
    let config = config_of(&sandbox, 1);
    let refusing = Server::with_offload(
        config,
        sandbox.services(),
        quiet_ports(),
        Arc::new(Refusing),
    );
    for server in [panicking, refusing] {
        let mut wire = Wire::start(server);
        wire.initialized("2025-06-18").await;
        for id in [2, 3] {
            wire.send(&find(id)).await;
            let answer = parsed(&wire.line().await);
            assert_eq!(answer["result"]["content"][0]["text"], UNPLANNED);
            assert_eq!(answer["result"]["isError"], true);
        }
    }
}
