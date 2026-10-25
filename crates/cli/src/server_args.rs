//! The agentless-mcp-server command line: its parser, its start-up refusals, and the arguments it
//! gives the MCP server.

use std::ffi::OsString;
use std::net::{IpAddr, ToSocketAddrs};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use app::{EnvConfig, IntArg};
use text::CodePointString;

use crate::Streams;
use crate::args::{self, Action, Arg, Exit, Kind, Namespace, Parser, TEXT, Value};
use crate::args::{float_text, float_value, help_action, int_value};
use crate::exit::{EXIT_OK, EXIT_USAGE, report, write_answer};
use crate::steps::columns;

const PROG: &str = "agentless-mcp-server";
const DESCRIPTION: &str = "Read-only MCP server over the agentless-mcp read surface.";
const VERSION: &str = concat!("agentless-mcp-server ", env!("CARGO_PKG_VERSION"));
const VERSION_HELP: &str = "show program's version number and exit";
const ROOT_HELP: &str = "a repository this server may serve; must exist at startup; repeatable";
const ROOTS_FROM_HELP: &str = "a file of repository paths, one per line, added to --root; blank \
     lines and #-comment lines are skipped and relative paths resolve against the working \
     directory, as --root does; repeatable";
// The flag is accepted and has no effect; the reference's help described the effect it had there.
const CLIENT_ROOTS_HELP: &str = "accepted for compatibility and has no effect: only --root and \
     --roots-from name the repositories this server may serve";
const NO_AUTO_WARM_HELP: &str = "do not warm cold grammars in the background at startup; \
     grammars then warm only through agentless-mcp warmup (AGENTLESS_MCP_NO_AUTO_WARM in the \
     environment does the same)";
const NO_AUTO_INDEX_HELP: &str = "do not refresh a stale tag cache in the background when a \
     repository is first served; the cache then updates only through agentless-mcp index \
     (AGENTLESS_MCP_NO_AUTO_INDEX in the environment does the same)";
const NO_AUTO_RESTART_HELP: &str = "do not restart the HTTP server when its installed package \
     changes; the process then serves the code it loaded at startup until restarted by hand \
     (AGENTLESS_MCP_NO_AUTO_RESTART in the environment does the same)";
// DV2: the reference help named the removed surfaces and the framework of the Python server.
const SURFACE_HELP: &str = "which tool surface to publish: v2, the seven consolidated \
     intent-shaped tools, is the only one. v1 and both were removed in 0.11.0 and are refused at \
     startup";
const TRANSPORT_HELP: &str = "how the server talks to its client: stdio (the default) for a \
     client that launches this process, http for one long-lived server several clients share \
     over the streamable-http transport";
const HOST_HELP: &str = "address --transport http binds; loopback only, default 127.0.0.1";
const PORT_HELP: &str = "port --transport http binds; 1-65535, default 8000";
const CONCURRENCY_HELP: &str = "how many tool handlers may hold worker threads at once; 1-64, \
     default 8. Raise it for a --transport http server several clients share";
const CALL_LIMIT_HELP: &str = "how long one tool call may work before it stops with a refusal; \
     a call the client cancels also stops. Default 30";
const GRACE_HELP: &str = "extra time a call may spend parsing files the tag cache does not \
     hold, on top of --call-limit. Default 60";
const SURFACES: &[&str] = &["v1", "v2", "both"];
const SURFACE: &str = "v2";
const TRANSPORTS: &[&str] = &["stdio", "http"];
const STDIO: &str = "stdio";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8000;
const DEFAULT_CONCURRENCY: usize = 8;
const MAX_CONCURRENCY: usize = 64;
const DEFAULT_CALL_LIMIT: f64 = 30.0;
const DEFAULT_GRACE: f64 = 60.0;

/// The arguments of one agentless-mcp-server command line.
#[derive(Debug)]
pub struct ServerArgs<R> {
    /// Each `--root`, with `~` expanded and resolved, in command-line order.
    pub roots: Vec<PathBuf>,
    /// What the roots-file reader returned for each `--roots-from`, in command-line order.
    pub roots_from: Vec<R>,
    /// `--no-auto-index`.
    pub no_auto_index: bool,
    /// `--no-auto-restart`.
    pub no_auto_restart: bool,
    /// The transport and, for HTTP, the address to listen on.
    pub transport: ServerTransport,
    /// How many calls may work at once.
    pub max_concurrency: usize,
    /// The seconds of work a call may do.
    pub call_limit: f64,
    /// The most seconds of uncached parsing that a call may add to its limit.
    pub recache_grace: f64,
}

/// How the server talks to its client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerTransport {
    /// Standard input and output.
    Stdio,
    /// Streamable HTTP on one loopback address.
    Http {
        /// The address that the host check resolved, which the server binds.
        host: IpAddr,
        /// The port.
        port: u16,
    },
}

/// Parses an agentless-mcp-server command line, given without the program name. `read_roots`
/// reads each `--roots-from` file at its place in the command line, or gives its refusal. A help,
/// a version or a refused line is written to `streams`, and its exit code is returned instead.
///
/// # Errors
///
/// Returns 0 after the help or version text, and 2 after a usage error.
pub fn server_args<R>(
    args: &[OsString],
    env: &EnvConfig,
    streams: &mut Streams<'_>,
    read_roots: &mut dyn FnMut(&str) -> Result<R, String>,
) -> Result<ServerArgs<R>, u8> {
    let given: Vec<Arg> = args.iter().map(|arg| Arg::new(arg)).collect();
    let width = columns(env) - 2;
    let parser = parser();
    let mut files = Vec::new();
    let namespace = args::parse_alone_checked(&parser, &given, width, &mut |dest, value| {
        checked(dest, value, env, &mut files, read_roots)
    });
    match namespace.and_then(|namespace| settled(&parser, width, &namespace, files)) {
        Ok(server) => Ok(server),
        Err(Exit::Help(text)) => Err(write_answer(streams, &text, "", EXIT_OK)),
        Err(Exit::Usage(text)) => {
            report(streams.err, &text);
            report(streams.err, &argv_report(&given));
            Err(EXIT_USAGE)
        }
    }
}

fn parser() -> Parser {
    let version = Action {
        strings: &["--version"],
        dest: "version",
        metavar: None,
        help: Some(VERSION_HELP.to_owned()),
        kind: Kind::Version(VERSION),
        required: false,
    };
    Parser {
        prog: PROG.to_owned(),
        description: Some(DESCRIPTION),
        actions: vec![
            help_action(),
            version,
            listed(&["--root"], "root", "DIR", ROOT_HELP),
            listed(&["--roots-from"], "roots_from", "FILE", ROOTS_FROM_HELP),
            // Accepted, with no effect: the server asks for no client roots and links each grammar.
            flag(
                &["--allow-client-roots"],
                "allow_client_roots",
                CLIENT_ROOTS_HELP,
            ),
            flag(&["--no-auto-warm"], "no_auto_warm", NO_AUTO_WARM_HELP),
            flag(&["--no-auto-index"], "no_auto_index", NO_AUTO_INDEX_HELP),
            flag(
                &["--no-auto-restart"],
                "no_auto_restart",
                NO_AUTO_RESTART_HELP,
            ),
            chosen(&["--surface"], "surface", SURFACES, SURFACE_HELP),
            chosen(&["--transport"], "transport", TRANSPORTS, TRANSPORT_HELP),
            valued(&["--host"], "host", "ADDR", HOST_HELP, TEXT),
            valued(&["--port"], "port", "N", PORT_HELP, int_value),
            valued(
                &["--max-concurrency"],
                "max_concurrency",
                "N",
                CONCURRENCY_HELP,
                int_value,
            ),
            valued(
                &["--call-limit"],
                "call_limit",
                "SECONDS",
                CALL_LIMIT_HELP,
                float_value,
            ),
            valued(
                &["--recache-grace"],
                "recache_grace",
                "SECONDS",
                GRACE_HELP,
                float_value,
            ),
        ],
        group: None,
    }
}

fn flag(strings: &'static [&'static str], dest: &'static str, help: &str) -> Action {
    Action {
        strings,
        dest,
        metavar: None,
        help: Some(help.to_owned()),
        kind: Kind::Flag,
        required: false,
    }
}

fn valued(
    strings: &'static [&'static str],
    dest: &'static str,
    metavar: &'static str,
    help: &str,
    convert: args::Convert,
) -> Action {
    Action {
        metavar: Some(metavar),
        kind: Kind::Value {
            convert,
            choices: &[],
            repeat: false,
        },
        ..flag(strings, dest, help)
    }
}

fn listed(
    strings: &'static [&'static str],
    dest: &'static str,
    metavar: &'static str,
    help: &str,
) -> Action {
    Action {
        metavar: Some(metavar),
        kind: Kind::Value {
            convert: TEXT,
            choices: &[],
            repeat: true,
        },
        ..flag(strings, dest, help)
    }
}

fn chosen(
    strings: &'static [&'static str],
    dest: &'static str,
    choices: &'static [&'static str],
    help: &str,
) -> Action {
    Action {
        kind: Kind::Value {
            convert: TEXT,
            choices,
            repeat: false,
        },
        ..flag(strings, dest, help)
    }
}

// The conversions that read the filesystem run in the parse, at their place in the command line.
fn checked<R>(
    dest: &'static str,
    value: Value,
    env: &EnvConfig,
    files: &mut Vec<R>,
    read_roots: &mut dyn FnMut(&str) -> Result<R, String>,
) -> Result<Value, String> {
    match (dest, value) {
        ("root", Value::Text(arg)) => {
            root_dir(&arg, env).map(|root| Value::Text(Arg::new(root.as_os_str())))
        }
        ("roots_from", Value::Text(arg)) => {
            files.push(read_roots(&arg.utf8("--roots-from")?)?);
            Ok(Value::Text(arg))
        }
        (_, value) => Ok(value),
    }
}

// A path that is not UTF-8 is refused (ledger deviation DV6); a leading ~name is a relative path,
// never a user (DV7).
fn root_dir(arg: &Arg, env: &EnvConfig) -> Result<PathBuf, String> {
    let raw = arg.utf8("--root")?;
    let expanded = app::expand_user(Path::new(&raw), env.var("HOME"));
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        let cwd = env
            .cwd()
            .map_err(|unread| format!("--root {raw}: {}", io::os_error_str(&unread.error())))?;
        cwd.join(expanded)
    };
    let resolved = io::resolve(&absolute)
        .map_err(|error| format!("--root {raw}: {}", io::os_error_text(&error, &absolute)))?;
    if !resolved.is_dir() {
        let shown = resolved.display();
        return Err(format!("--root {raw} is not a directory: {shown}"));
    }
    Ok(resolved)
}

// The reference's checks in their order, then the refusal of the removed surfaces (DV3).
fn settled<R>(
    parser: &Parser,
    width: isize,
    namespace: &Namespace,
    roots_from: Vec<R>,
) -> Result<ServerArgs<R>, Exit> {
    let refused = |message: String| args::error(parser, width, &message);
    let transport = transport(namespace).map_err(refused)?;
    let max_concurrency = concurrency(namespace).map_err(refused)?;
    let (call_limit, recache_grace) = limits(namespace).map_err(refused)?;
    surface(namespace).map_err(refused)?;
    Ok(ServerArgs {
        roots: namespace
            .list("root")
            .iter()
            .map(|arg| PathBuf::from(&arg.raw))
            .collect(),
        roots_from,
        no_auto_index: namespace.flag("no_auto_index"),
        no_auto_restart: namespace.flag("no_auto_restart"),
        transport,
        max_concurrency,
        call_limit,
        recache_grace,
    })
}

fn transport(namespace: &Namespace) -> Result<ServerTransport, String> {
    let host = namespace.text("host").map(|arg| arg.text.as_str());
    let port = match namespace.get("port") {
        Some(Value::Int(port)) => Some(port),
        _ => None,
    };
    if namespace
        .text("transport")
        .is_none_or(|arg| arg.text == STDIO)
    {
        return stdio_only(host.is_some(), port.is_some());
    }
    if namespace.flag("allow_client_roots") {
        return Err(CLIENT_ROOTS_OVER_HTTP.to_owned());
    }
    let port = port_number(port)?;
    let host = host.unwrap_or(DEFAULT_HOST);
    let host = loopback_literal(host).ok_or_else(|| not_loopback(host))?;
    Ok(ServerTransport::Http { host, port })
}

const CLIENT_ROOTS_OVER_HTTP: &str = "--allow-client-roots cannot be combined with --transport \
     http. That flag lets the connected client's advertised roots authorise repositories, which \
     is safe under stdio because the client is the process that spawned this server. Over HTTP \
     the client is whatever reaches the port, and loopback is not per-user isolated, so any \
     local process could name its own root and read anything this server's user can read -- \
     the --root allowlist would stop deciding what is servable. Drop --allow-client-roots and \
     enrol the repositories with --root or --roots-from.";

// --host and --port have no socket to bind under stdio.
fn stdio_only(host: bool, port: bool) -> Result<ServerTransport, String> {
    let given: Vec<&str> = [("--host", host), ("--port", port)]
        .into_iter()
        .filter_map(|(flag, given)| given.then_some(flag))
        .collect();
    if given.is_empty() {
        return Ok(ServerTransport::Stdio);
    }
    Err(format!(
        "{} apply to --transport http and the transport is stdio, which has no socket to bind. \
         Pass --transport http to serve over HTTP, or drop the flag.",
        given.join(" and ")
    ))
}

fn port_number(port: Option<&IntArg>) -> Result<u16, String> {
    let Some(port) = port else {
        return Ok(DEFAULT_PORT);
    };
    let number = port.value().and_then(|value| u16::try_from(value).ok());
    number.filter(|&number| number >= 1).ok_or_else(|| {
        format!(
            "--port {port} is outside 1-65535. Port 0 binds an ephemeral port this process never \
             reports, and anything above the range fails inside the transport as an opaque bind \
             error. Name a port a client can be registered against."
        )
    })
}

// One lookup: the address it gives first is the one the server binds, so the check and the bind
// cannot disagree. Every address must be loopback, an IPv4-mapped one by its IPv4 form.
fn loopback_literal(host: &str) -> Option<IpAddr> {
    let addresses: Vec<IpAddr> = (host, 0).to_socket_addrs().ok()?.map(|a| a.ip()).collect();
    let first = *addresses.first()?;
    let loopback = addresses.iter().all(|ip| ip.to_canonical().is_loopback());
    loopback.then_some(first)
}

fn not_loopback(host: &str) -> String {
    let shown = CodePointString::from(host).quoted();
    format!(
        "--host {shown} is not a loopback address. This server authenticates no one: the --root \
         allowlist decides which repositories are readable, not who may read them, so binding it \
         where the network can reach it publishes every enrolled repository. Bind \
         {DEFAULT_HOST} and put an authenticating proxy in front if you need it off-host."
    )
}

fn concurrency(namespace: &Namespace) -> Result<usize, String> {
    let Some(Value::Int(count)) = namespace.get("max_concurrency") else {
        return Ok(DEFAULT_CONCURRENCY);
    };
    let value = count
        .value()
        .filter(|value| (1..=MAX_CONCURRENCY).contains(value));
    value.ok_or_else(|| {
        format!(
            "--max-concurrency {count} is outside 1-{MAX_CONCURRENCY}. Zero or less admits no \
             handler and the server would answer nothing; above the range the gate stops bounding \
             the tag-cache connections and git children a shared server holds at once."
        )
    })
}

fn limits(namespace: &Namespace) -> Result<(f64, f64), String> {
    let seconds = |dest: &str, default: f64| {
        let text = namespace.text(dest).map(|arg| arg.text.as_str());
        text.and_then(float_text).unwrap_or(default)
    };
    let (limit, grace) = (
        seconds("call_limit", DEFAULT_CALL_LIMIT),
        seconds("recache_grace", DEFAULT_GRACE),
    );
    if limit <= 0.0 {
        let shown = json::general(limit);
        return Err(format!(
            "--call-limit {shown} is not positive. Every call would stop at its first checkpoint, \
             so the server would answer nothing."
        ));
    }
    if grace < 0.0 {
        let shown = json::general(grace);
        return Err(format!(
            "--recache-grace {shown} is negative. Use 0 for no grace."
        ));
    }
    Ok((limit, grace))
}

// DV3: 0.11.0 serves the v2 tools only.
fn surface(namespace: &Namespace) -> Result<(), String> {
    match namespace.text("surface") {
        Some(arg) if arg.text != SURFACE => Err(format!(
            "--surface {} is no longer served: 0.11.0 removed the v1 tools. Drop --surface, or \
             pass --surface v2. The v1-to-v2 mapping table is in the agent guide: agentless-mcp \
             guide --section the-two-surfaces.",
            arg.text
        )),
        _ => Ok(()),
    }
}

// Under an MCP client a usage error reads as a closed connection, so the argv is the diagnostic.
fn argv_report(argv: &[Arg]) -> String {
    let shown: Vec<String> = argv
        .iter()
        .map(|arg| CodePointString::from_utf8_escaped(arg.raw.as_bytes()).quoted())
        .collect();
    let received = format!("{PROG}: received argv: [{}]\n", shown.join(", "));
    let glued = argv
        .iter()
        .enumerate()
        .filter(|(_, arg)| unsplit(&arg.text));
    let notes = glued.map(|(index, _)| {
        format!(
            "{PROG}: argv element {index} looks like an unsplit shell string; check quoting in \
             the client registration. Pass each token as its own argv element (e.g. \"--root\", \
             \"/a\", \"--root\", \"/b\"), or use --roots-from FILE.\n"
        )
    });
    std::iter::once(received).chain(notes).collect()
}

// An option glued to its values: a shell string that was never split. A lex error is no evidence.
fn unsplit(element: &str) -> bool {
    text::split_shell_words(element)
        .is_ok_and(|tokens| tokens.len() > 1 && tokens[0].starts_with('-'))
}

#[cfg(test)]
mod tests;
