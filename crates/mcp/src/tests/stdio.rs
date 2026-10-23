use std::time::Duration;

use serde_json::{Value as Json, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

use super::Sandbox;
use crate::Ended;
use crate::stdio::serve;

const LIMIT: Duration = Duration::from_secs(20);
const BROKEN: &str = "agentless-mcp-server: cannot write to standard output: OSError: broken pipe";

async fn sent(input: &mut DuplexStream, message: &Json) {
    let line = format!("{message}\n");
    input.write_all(line.as_bytes()).await.expect("a sent line");
}

// Failure injection: the client reads the initialize answer, then closes its end of the output
// while its input stays open. The next answer's write fails, and that failure ends the session.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_write_to_the_output_ends_the_session() {
    let sandbox = Sandbox::new();
    sandbox.step(&["write", "repo/a.txt", "a\n"]);
    let server = sandbox.server(&["repo"], &[], None);
    let (mut input, server_input) = tokio::io::duplex(1 << 16);
    let (output, server_output) = tokio::io::duplex(1 << 16);
    let serving = tokio::spawn(serve(server, (server_input, server_output)));
    let params = json!({"protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "probe", "version": "0"}});
    sent(
        &mut input,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params}),
    )
    .await;
    let mut lines = BufReader::new(output).lines();
    let read = tokio::time::timeout(LIMIT, lines.next_line()).await;
    let answer = read
        .expect("an answer")
        .expect("a line")
        .expect("an open output");
    assert!(answer.contains(r#""id":1"#), "{answer}");
    drop(lines);
    sent(
        &mut input,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    sent(
        &mut input,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
    )
    .await;
    let ended = tokio::time::timeout(LIMIT, serving).await;
    let ended = ended
        .expect("an end before the limit")
        .expect("a serve task");
    assert_eq!(ended, Ended::Unwritable(BROKEN.to_owned()));
    drop(input);
}

// The control: with its output open, the session answers and ends only with its input.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_with_its_output_open_ends_with_its_input() {
    let sandbox = Sandbox::new();
    sandbox.step(&["write", "repo/a.txt", "a\n"]);
    let server = sandbox.server(&["repo"], &[], None);
    let (mut input, server_input) = tokio::io::duplex(1 << 16);
    let (output, server_output) = tokio::io::duplex(1 << 16);
    let serving = tokio::spawn(serve(server, (server_input, server_output)));
    let params = json!({"protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "probe", "version": "0"}});
    sent(
        &mut input,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params}),
    )
    .await;
    sent(
        &mut input,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    sent(
        &mut input,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
    )
    .await;
    let mut lines = BufReader::new(output).lines();
    for id in [r#""id":1"#, r#""id":2"#] {
        let read = tokio::time::timeout(LIMIT, lines.next_line()).await;
        let answer = read
            .expect("an answer")
            .expect("a line")
            .expect("an open output");
        assert!(answer.contains(id), "{answer}");
    }
    drop(input);
    let ended = tokio::time::timeout(LIMIT, serving).await;
    assert_eq!(
        ended
            .expect("an end before the limit")
            .expect("a serve task"),
        Ended::Closed
    );
}
