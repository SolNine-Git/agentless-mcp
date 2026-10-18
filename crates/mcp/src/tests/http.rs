use std::convert::Infallible;
use std::future::Ready;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::service::{HttpService, service_fn};
use hyper::{Request, Response};
use hyper_util::server::graceful::GracefulShutdown;
use serde_json::{Value as Json, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::Sandbox;
use crate::Server;
use crate::http::{connection, routed, service};

const READ_LIMIT: Duration = Duration::from_secs(20);
const BOUND: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const HOST_REFUSED: (&str, &str) = (
    "HTTP/1.1 403 Forbidden",
    "Forbidden: Host header is not allowed",
);
const INITIALIZE: &str = concat!(
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","#,
    r#""capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}"#,
);

fn server() -> (Sandbox, Server) {
    let sandbox = Sandbox::new();
    sandbox.step(&["write", "repo/a.txt", "a\n"]);
    let server = sandbox.server(&["repo"], &[], None);
    let server = Server {
        over_http: true,
        ..server
    };
    (sandbox, server)
}

// One request on its own connection, which the request asks to close after the answer.
async fn exchange<S>(service: S, request: &str) -> String
where
    S: HttpService<Incoming> + Send + 'static,
    S::Future: Send + 'static,
    S::ResBody: Send + 'static,
    <S::ResBody as hyper::body::Body>::Data: Send,
    <S::ResBody as hyper::body::Body>::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let (mut client, served) = tokio::io::duplex(1 << 20);
    let graceful = GracefulShutdown::new();
    connection(served, service, &graceful);
    client
        .write_all(request.as_bytes())
        .await
        .expect("a sent request");
    let mut answer = Vec::new();
    let read = tokio::time::timeout(READ_LIMIT, client.read_to_end(&mut answer)).await;
    drop(read.expect("an answer before the limit"));
    String::from_utf8(answer).expect("a UTF-8 answer")
}

fn post(path: &str, host: &str, body: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAccept: \
         application/json, text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\
         \r\n{body}",
        body.len()
    )
}

fn with_origin(request: &str, origin: &str) -> String {
    request.replacen("\r\n", &format!("\r\nOrigin: {origin}\r\n"), 1)
}

// The status line and the body of an answer that carries no event stream.
fn status_and_body(answer: &str) -> (&str, &str) {
    let (head, body) = answer.split_once("\r\n\r\n").expect("a head and a body");
    (head.lines().next().unwrap_or_default(), body)
}

fn assert_initialized(answer: &str, case: &str) {
    assert!(
        answer.starts_with("HTTP/1.1 200 OK\r\n"),
        "{case}: {answer}"
    );
    let result = &message(answer)["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18", "{case}");
}

// The JSON-RPC message of the one SSE event of an answer.
fn message(answer: &str) -> Json {
    let data = answer
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap_or_else(|| panic!("no event data: {answer}"));
    serde_json::from_str(data).expect("a JSON message")
}

// Over HTTP the reference says that its prompt and resource lists can change; logging is left out
// (deviation D2).
fn http_capabilities() -> Json {
    json!({
        "experimental": {},
        "prompts": {"listChanged": true},
        "resources": {"subscribe": false, "listChanged": true},
        "tools": {"listChanged": true},
        "extensions": {"io.modelcontextprotocol/ui": {}},
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn both_mcp_paths_answer_initialize_on_a_loopback_host() {
    let (_sandbox, server) = server();
    let (mcp, _sessions) = service(&server, BOUND);
    for (path, host) in [
        ("/mcp", "127.0.0.1:8766"),
        ("/mcp/", "localhost:8766"),
        ("/mcp", "[::1]:8766"),
        ("/mcp/", "localhost"),
        ("/mcp", "LOCALHOST:8766"),
        ("/mcp", "127.0.0.1:9999"),
    ] {
        let answer = exchange(routed(mcp.clone()), &post(path, host, INITIALIZE)).await;
        assert_initialized(&answer, host);
        let result = &message(&answer)["result"];
        assert_eq!(result["capabilities"], http_capabilities(), "{path} {host}");
    }
}

// The DNS-rebinding guard (deviation): the reference serves any Host.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_that_is_not_an_allowed_name_is_refused() {
    let (_sandbox, server) = server();
    let (mcp, _sessions) = service(&server, BOUND);
    for host in [
        "evil.example",
        "evil.example:8766",
        "localhost.evil.example:8766",
        "127.0.0.2:8766",
    ] {
        let answer = exchange(routed(mcp.clone()), &post("/mcp", host, INITIALIZE)).await;
        assert_eq!(status_and_body(&answer), HOST_REFUSED, "{host}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bind_literal_is_an_allowed_host() {
    let (_sandbox, server) = server();
    let mapped = IpAddr::V6(Ipv4Addr::LOCALHOST.to_ipv6_mapped());
    for (bound, host) in [
        (IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), "127.0.0.2:8766"),
        (mapped, "[::ffff:127.0.0.1]:8766"),
    ] {
        let (mcp, _sessions) = service(&server, bound);
        let answer = exchange(routed(mcp), &post("/mcp", host, INITIALIZE)).await;
        assert_initialized(&answer, host);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_host_keeps_its_bad_request_answer() {
    let (_sandbox, server) = server();
    let (mcp, _sessions) = service(&server, BOUND);
    let answer = exchange(routed(mcp), &post("/mcp", "bad host!", INITIALIZE)).await;
    let refused = (
        "HTTP/1.1 400 Bad Request",
        "Bad Request: Invalid Host header",
    );
    assert_eq!(status_and_body(&answer), refused, "{answer}");
    assert!(
        answer.contains("content-type: text/plain; charset=utf-8\r\n"),
        "{answer}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_loopback_origin_or_no_origin_is_served() {
    let (_sandbox, server) = server();
    let (mcp, _sessions) = service(&server, BOUND);
    let plain = post("/mcp", "127.0.0.1:8766", INITIALIZE);
    let answer = exchange(routed(mcp.clone()), &plain).await;
    assert_initialized(&answer, "no Origin");
    for origin in [
        "http://127.0.0.1:8766",
        "https://localhost",
        "http://localhost:3000",
        "http://[::1]:8766",
    ] {
        let answer = exchange(routed(mcp.clone()), &with_origin(&plain, origin)).await;
        assert_initialized(&answer, origin);
    }
}

// The DNS-rebinding guard (deviation): the reference serves any Origin.
#[tokio::test(flavor = "multi_thread")]
async fn an_origin_that_is_not_loopback_is_refused() {
    let (_sandbox, server) = server();
    let (mcp, _sessions) = service(&server, BOUND);
    let plain = post("/mcp", "127.0.0.1:8766", INITIALIZE);
    let not_allowed = "Forbidden: Origin header is not allowed";
    for (origin, body) in [
        ("http://evil.example", not_allowed),
        ("http://localhost.evil.example:8766", not_allowed),
        ("ftp://localhost:8766", not_allowed),
        ("null", not_allowed),
        ("evil.example", "Forbidden: Invalid Origin header"),
    ] {
        let answer = exchange(routed(mcp.clone()), &with_origin(&plain, origin)).await;
        let refused = ("HTTP/1.1 403 Forbidden", body);
        assert_eq!(status_and_body(&answer), refused, "{origin}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn another_path_is_not_found_as_the_reference_answers() {
    let (_sandbox, server) = server();
    let (mcp, _sessions) = service(&server, BOUND);
    for path in ["/", "/other", "/mcpx", "/mcp/x"] {
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        let answer = exchange(routed(mcp.clone()), &request).await;
        assert!(answer.starts_with("HTTP/1.1 404 Not Found\r\n"), "{answer}");
        assert!(
            answer.contains("content-type: text/plain; charset=utf-8\r\n"),
            "{answer}"
        );
        assert!(answer.ends_with("\r\n\r\nNot Found"), "{answer}");
    }
}

fn panicked(_request: Request<Incoming>) -> Ready<Result<Response<Full<Bytes>>, Infallible>> {
    panic!("an injected panic in a connection");
}

// Failure injection: a service that panics ends its own connection only; the next connection is
// served.
#[tokio::test(flavor = "multi_thread")]
async fn a_panic_in_one_connection_leaves_the_next_served() {
    let (_sandbox, server) = server();
    let (mcp, _sessions) = service(&server, BOUND);
    let panicking = service_fn(panicked);
    let request = "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    assert_eq!(exchange(panicking, request).await, "");
    let answer = exchange(routed(mcp), &post("/mcp", "127.0.0.1", INITIALIZE)).await;
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
}
