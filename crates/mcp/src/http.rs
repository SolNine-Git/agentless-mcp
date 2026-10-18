//! Streamable HTTP: one rmcp service behind `/mcp` and `/mcp/` on hyper, the stop on a signal or a
//! due restart, and the drain of the calls before the end.

use std::convert::Infallible;
use std::fmt::Display;
use std::future::{Future, pending, poll_fn};
use std::net::{IpAddr, SocketAddr};
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Body, Bytes, Incoming};
use hyper::header::{CONTENT_TYPE, HeaderValue};
use hyper::server::conn::http1;
use hyper::service::{HttpService, service_fn};
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use hyper_util::server::graceful::GracefulShutdown;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::mpsc;

use crate::drain::drained;
use crate::restart::{Monitor, ticking};
use crate::serve::Ended;
use crate::server::{Log, Server};
use crate::service::first;

pub(crate) type Mcp = StreamableHttpService<Server, LocalSessionManager>;
pub(crate) type AnswerBody = BoxBody<Bytes, Infallible>;
pub(crate) type Answer = Response<AnswerBody>;

const PATHS: [&str; 2] = ["/mcp", "/mcp/"];
const NOT_FOUND: &str = "Not Found";
const PLAIN_TEXT: &str = "text/plain; charset=utf-8";
// rmcp matches a Host entry that has no port against every port, so a loopback name passes with
// any port.
const LOOPBACK_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];
const LOOPBACK_ORIGINS: [&str; 6] = [
    "http://localhost:*",
    "https://localhost:*",
    "http://127.0.0.1:*",
    "https://127.0.0.1:*",
    "http://[::1]:*",
    "https://[::1]:*",
];

// Why the server stopped accepting: the number of the signal that stopped it, or a due restart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    Signal(i32),
    Restart,
}

// SIGINT and SIGTERM, which stop the server as the reference's graceful shutdown does.
pub(crate) struct Signals {
    interrupt: Signal,
    terminate: Signal,
}

impl Signals {
    fn new() -> std::io::Result<Signals> {
        Ok(Signals {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    // The number of the next signal that arrives.
    fn poll_next(&mut self, context: &mut Context<'_>) -> Poll<i32> {
        if self.interrupt.poll_recv(context).is_ready() {
            return Poll::Ready(SignalKind::interrupt().as_raw_value());
        }
        if self.terminate.poll_recv(context).is_ready() {
            return Poll::Ready(SignalKind::terminate().as_raw_value());
        }
        Poll::Pending
    }
}

pub(crate) async fn serve(
    server: Server,
    address: SocketAddr,
    monitor: Option<Monitor>,
    log: Arc<dyn Log>,
) -> Ended {
    let listener = match TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(error) => {
            let error = io::os_error_line(&error);
            return Ended::Failed(format!(
                "agentless-mcp-server: cannot listen on {address}: {error}"
            ));
        }
    };
    let mut signals = match Signals::new() {
        Ok(signals) => signals,
        Err(error) => {
            let error = io::os_error_line(&error);
            return Ended::Failed(format!(
                "agentless-mcp-server: cannot watch signals: {error}"
            ));
        }
    };
    log.line(&format!(
        "agentless-mcp-server: serving MCP at http://{address}/mcp"
    ));
    let server = Server {
        over_http: true,
        ..server
    };
    let (mcp, sessions) = service(&server, address.ip());
    let graceful = GracefulShutdown::new();
    let stop = accepted(
        &listener,
        &mcp,
        &graceful,
        stopped(monitor.as_ref(), &mut signals),
        &*log,
    )
    .await;
    drop(listener);
    let bound = Duration::try_from_secs_f64(server.inner.call_limit + server.inner.recache_grace);
    let mut expired = pin!(expiry(bound.ok(), &mut signals));
    let calls_ended = drained(&server.gate, server.permits, expired.as_mut()).await;
    sessions();
    if calls_ended {
        first(graceful.shutdown(), expired.as_mut()).await;
    }
    match stop {
        Stop::Signal(number) => Ended::Signalled(number),
        Stop::Restart => Ended::Relaunch,
    }
}

// The rmcp service over clones of `server`, and the cancel of its sessions. It refuses a Host
// or Origin that is not loopback, and sends no SSE keep-alive or priming event (as the reference).
pub(crate) fn service(server: &Server, bound: IpAddr) -> (Mcp, impl FnOnce()) {
    let hosts = LOOPBACK_HOSTS.map(String::from).into_iter();
    let config = StreamableHttpServerConfig::default()
        .with_allowed_hosts(hosts.chain([bind_literal(bound)]))
        .with_allowed_origins(LOOPBACK_ORIGINS)
        .with_sse_keep_alive(None)
        .with_sse_retry(None);
    let sessions = config.cancellation_token.clone();
    let server = server.clone();
    let mcp = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    (mcp, move || sessions.cancel())
}

// The bound address as a Host names it: an IPv6 literal in brackets.
fn bind_literal(bound: IpAddr) -> String {
    match bound {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

// Accepts connections until the stop finishes, and returns why it stopped. A failed accept is
// logged, and the loop goes on.
async fn accepted(
    listener: &TcpListener,
    mcp: &Mcp,
    graceful: &GracefulShutdown,
    stop: impl Future<Output = Stop>,
    log: &dyn Log,
) -> Stop {
    let mut stop = pin!(stop);
    loop {
        let next = poll_fn(|context| {
            if let Poll::Ready(stop) = stop.as_mut().poll(context) {
                return Poll::Ready(Err(stop));
            }
            listener.poll_accept(context).map(Ok)
        })
        .await;
        match next {
            Err(stop) => return stop,
            Ok(Ok((stream, _))) => connection(stream, routed(mcp.clone()), graceful),
            Ok(Err(error)) => log.line(&format!(
                "agentless-mcp-server: accept failed: {}",
                io::os_error_line(&error)
            )),
        }
    }
}

// The first of a signal and a due restart.
async fn stopped(monitor: Option<&Monitor>, signals: &mut Signals) -> Stop {
    let mut restart = pin!(restart_due(monitor));
    poll_fn(|context| {
        if restart.as_mut().poll(context).is_ready() {
            return Poll::Ready(Stop::Restart);
        }
        signals.poll_next(context).map(Stop::Signal)
    })
    .await
}

// Ends when the monitor asks for a restart; without a monitor, never.
async fn restart_due(monitor: Option<&Monitor>) {
    let Some(monitor) = monitor else {
        return pending().await;
    };
    let (sender, ticks) = mpsc::channel(1);
    tokio::spawn(ticking(sender));
    if !monitor.watch(ticks).await {
        pending::<()>().await;
    }
}

// The bound of the drain, or a second signal, whichever comes first. No bound waits for the signal.
async fn expiry(bound: Option<Duration>, signals: &mut Signals) {
    let timer = async {
        match bound {
            Some(bound) => tokio::time::sleep(bound).await,
            None => pending().await,
        }
    };
    let second = poll_fn(|context| signals.poll_next(context).map(drop));
    first(timer, pin!(second)).await;
}

// One connection on its own task, watched for the graceful end. A panic in it ends that task only.
pub(crate) fn connection<IO, S>(io: IO, service: S, graceful: &GracefulShutdown)
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: HttpService<Incoming> + Send + 'static,
    S::Future: Send + 'static,
    S::ResBody: Send + 'static,
    <S::ResBody as Body>::Data: Send,
    <S::ResBody as Body>::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let served = http1::Builder::new().serve_connection(TokioIo::new(io), service);
    let watched = graceful.watch(served);
    tokio::spawn(async move { drop(watched.await) });
}

// The MCP paths go to rmcp, which reads no path; every other path is not found, as the
// reference's router answers.
pub(crate) fn routed(
    mcp: Mcp,
) -> impl HttpService<Incoming, ResBody = AnswerBody, Error = Infallible, Future: Send> + Clone {
    service_fn(move |request: Request<Incoming>| {
        let mcp = mcp.clone();
        async move { Ok::<_, Infallible>(answer(&mcp, request).await) }
    })
}

pub(crate) async fn answer<B>(mcp: &Mcp, request: Request<B>) -> Answer
where
    B: Body + Send + 'static,
    B::Error: Display,
{
    if PATHS.contains(&request.uri().path()) {
        return mcp.handle(request).await;
    }
    let mut answer = Response::new(Full::new(Bytes::from_static(NOT_FOUND.as_bytes())).boxed());
    *answer.status_mut() = StatusCode::NOT_FOUND;
    let plain = HeaderValue::from_static(PLAIN_TEXT);
    answer.headers_mut().insert(CONTENT_TYPE, plain);
    answer
}
