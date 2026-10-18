use std::ffi::OsString;
use std::net::IpAddr;
use std::path::Path;

use io::testing::Tree;
use serde_json::{Value, json};

use super::*;

const SANDBOX: &str = "<SANDBOX>";
const LOOPBACK: &str = "<LOOPBACK>";

fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("not a text: {value}"))
}

// The fixture's layout below a new scratch directory, whose resolved path stands for <SANDBOX>.
fn sandbox(layout: &[Value]) -> (Tree, String) {
    let tree = Tree::new().expect("a scratch directory");
    let root = io::resolve(tree.path()).expect("a resolvable scratch directory");
    let shown = root.display().to_string();
    for entry in layout {
        if let Some(dir) = entry["dir"].as_str() {
            tree.create_dir(dir).expect("a directory");
        } else if let Some(link) = entry["link"].as_str() {
            let target = root.join(text(&entry["to"]));
            tree.symlink(link, target).expect("a link");
        } else {
            let hex = text(&entry["hex"]);
            let bytes: Vec<u8> = (0..hex.len())
                .step_by(2)
                .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex digits"))
                .collect();
            let file = text(&entry["file"]);
            tree.write_file(file, placed(&bytes, &shown))
                .expect("a file");
        }
    }
    (tree, shown)
}

// The bytes with each <SANDBOX> written as the sandbox's path.
fn placed(bytes: &[u8], root: &str) -> Vec<u8> {
    let mark = SANDBOX.as_bytes();
    let mut done = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some(at) = rest.windows(mark.len()).position(|window| window == mark) {
        done.extend_from_slice(&rest[..at]);
        done.extend_from_slice(root.as_bytes());
        rest = &rest[at + mark.len()..];
    }
    done.extend_from_slice(rest);
    done
}

fn env_for(run: &Value, root: &str) -> EnvConfig {
    let mut vars = vec![(
        OsString::from("HOME"),
        OsString::from(format!("{root}/home")),
    )];
    if let Some(columns) = run["columns"].as_str() {
        vars.push((OsString::from("COLUMNS"), OsString::from(columns)));
    }
    EnvConfig::new(vars, Path::new(root).join("work"))
}

// A --host value that is not an IP literal goes to the platform resolver, and resolvers differ:
// glibc reads 0127.0.0.1 as 87.0.0.1, and macOS reads it as a loopback address.
fn resolves_a_name(run: &Value) -> bool {
    let argv: Vec<&str> = run["argv"]
        .as_array()
        .into_iter()
        .flatten()
        .map(text)
        .collect();
    argv.windows(2)
        .any(|pair| pair[0] == "--host" && pair[1].parse::<IpAddr>().is_err())
}

// The values of a parse in the fixture's form: a host that the resolver chose reads <LOOPBACK>
// once it is checked to be loopback.
fn parsed(args: &ServerArgs<RootsFile>, named: bool, root: &str) -> Value {
    let shown = |path: &Path| path.display().to_string().replace(root, SANDBOX);
    let (transport, host, port) = match args.transport {
        ServerTransport::Stdio => ("stdio", Value::Null, Value::Null),
        ServerTransport::Http { host, port } => {
            assert!(host.to_canonical().is_loopback(), "{host}");
            let host = if named {
                LOOPBACK.to_owned()
            } else {
                host.to_string()
            };
            ("http", json!(host), json!(port))
        }
    };
    json!({
        "roots": args.roots.iter().map(|root| shown(root)).collect::<Vec<_>>(),
        "roots_from": args.roots_from.iter().map(|file| shown(file.path())).collect::<Vec<_>>(),
        "no_auto_index": args.no_auto_index,
        "no_auto_restart": args.no_auto_restart,
        "transport": transport,
        "host": host,
        "port": port,
        "max_concurrency": args.max_concurrency,
        "call_limit": json::float_text(args.call_limit),
        "recache_grace": json::float_text(args.recache_grace),
    })
}

// Each run of the reference parser, under the Rust rules and the recorded deviations, with the
// real roots-file reader of the server.
#[test]
fn each_server_command_line_parses_as_the_reference_says() {
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/../cli");
    let layout: Vec<Value> = io::testing::cases(manifest, "server_args/parser", "layout");
    let runs: Vec<Value> = io::testing::cases(manifest, "server_args/parser", "runs");
    assert!(runs.len() > 100, "{} runs", runs.len());
    let resolved = runs.iter().filter(|run| resolves_a_name(run)).count();
    // 127.1, 0127.0.0.1, the empty name, localhost and LOCALHOST.
    assert_eq!(resolved, 5, "runs whose --host the resolver reads");
    for run in runs {
        let named = resolves_a_name(&run);
        // The reference ran on glibc, and the macOS resolver reads some of these names differently.
        if cfg!(target_os = "macos") && named {
            continue;
        }
        let (_tree, root) = sandbox(&layout);
        let argv: Vec<OsString> = run["argv"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|arg| OsString::from(text(arg).replace(SANDBOX, &root)))
            .collect();
        let env = env_for(&run, &root);
        let services = Services::new(&env);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let mut streams = cli::Streams {
            out: &mut out,
            err: &mut err,
        };
        let outcome = cli::server_args(&argv, &env, &mut streams, &mut |raw| {
            RootsFile::read(raw, &services).map_err(|error| error.to_string())
        });
        let (code, values) = match &outcome {
            Ok(server) => (0, parsed(server, named, &root)),
            Err(code) => (*code, Value::Null),
        };
        let shown = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace(&root, SANDBOX);
        let argv = &run["argv"];
        assert_eq!(Some(u64::from(code)), run["code"].as_u64(), "{argv}");
        assert_eq!(shown(&out), text(&run["out"]), "{argv}");
        assert_eq!(shown(&err), text(&run["err"]), "{argv}");
        assert_eq!(values, run["parsed"], "{argv}");
    }
}

#[test]
fn the_server_name_selects_the_door_however_the_path_reads() {
    assert!(invoked(OsStr::new("agentless-mcp-server")));
    assert!(invoked(OsStr::new("/opt/tool/bin/agentless-mcp-server")));
    assert!(!invoked(OsStr::new("agentless-mcp")));
    assert!(!invoked(OsStr::new("/bin/agentless-mcp-server-old")));
    assert!(!invoked(OsStr::new("")));
}
