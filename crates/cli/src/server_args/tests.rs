use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use super::*;

const USAGE_END: &str = "[--call-limit SECONDS] [--recache-grace SECONDS]\n";

fn run(args: &[OsString], env: &EnvConfig) -> (Result<ServerArgs<String>, u8>, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut streams = Streams {
        out: &mut out,
        err: &mut err,
    };
    let mut read = |raw: &str| {
        if raw.ends_with("bad") {
            Err(format!("cannot read roots file {raw}: refused"))
        } else {
            Ok(raw.to_owned())
        }
    };
    let parsed = server_args(args, env, &mut streams, &mut read);
    let shown = |bytes: Vec<u8>| String::from_utf8(bytes).expect("UTF-8 output");
    (parsed, shown(out), shown(err))
}

fn words(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn env() -> EnvConfig {
    EnvConfig::new(Vec::new(), PathBuf::from("/"))
}

// The error line follows the usage, and the argv report follows the error line.
fn refusal(err: &str) -> &str {
    let after = err.split_once(USAGE_END).map_or(err, |(_, after)| after);
    after.lines().next().unwrap_or_default()
}

#[test]
fn a_root_that_is_not_utf8_is_refused_by_name() {
    let raw = OsString::from_vec(b"/x\xff".to_vec());
    let (parsed, out, err) = run(&[OsString::from("--root"), raw], &env());
    assert_eq!(parsed.err(), Some(2));
    assert_eq!(out, "");
    let line =
        "agentless-mcp-server: error: argument --root: --root is not valid UTF-8: /x\u{fffd}";
    assert_eq!(refusal(&err), line);
    assert!(
        err.ends_with("received argv: ['--root', '/x\\udcff']\n"),
        "{err}"
    );
}

#[test]
fn the_reader_reads_each_roots_file_in_order() {
    let (parsed, _, err) = run(&words(&["--roots-from", "a", "--roots-from", "b"]), &env());
    assert_eq!(err, "");
    assert_eq!(parsed.expect("parsed").roots_from, ["a", "b"]);
}

#[test]
fn a_refused_roots_file_is_an_argument_error_at_its_place() {
    let (parsed, _, err) = run(&words(&["--roots-from", "bad", "--port", "x"]), &env());
    assert_eq!(parsed.err(), Some(2));
    let line = "agentless-mcp-server: error: argument --roots-from: cannot read roots file bad: \
                refused";
    assert_eq!(refusal(&err), line);
}

#[test]
fn a_relative_root_without_a_start_up_directory_names_the_os_error() {
    let missing = std::io::Error::from_raw_os_error(2);
    let env = EnvConfig::with_unread_cwd(Vec::new(), &missing);
    let (parsed, _, err) = run(&words(&["--root", "rel"]), &env);
    assert_eq!(parsed.err(), Some(2));
    let line = "agentless-mcp-server: error: argument --root: --root rel: [Errno 2] No such file \
                or directory";
    assert_eq!(refusal(&err), line);
}
