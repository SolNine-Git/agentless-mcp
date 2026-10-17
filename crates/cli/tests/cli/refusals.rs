//! The answers that only this build gives: the arguments and inputs it does not take, the
//! token counter it always has, the closed stdout, the panic line, and a repository that every
//! command but an in-place apply leaves as it found it.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStringExt;
use std::panic;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cli::Streams;

use crate::common::{Outcome, Sandbox};

fn run(sandbox: &Sandbox, args: &[&str]) -> Outcome {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    sandbox.run(&args, sandbox.path(), None)
}

fn refused(err: &str) -> Outcome {
    Outcome {
        out: String::new(),
        err: err.to_owned(),
        code: 2,
    }
}

// The cl100k_base encoding is part of the binary, so the flag never refuses (ledger deviation).
#[test]
fn a_tiktoken_run_answers_as_a_plain_run() {
    let sandbox = Sandbox::new();
    let counted = run(
        &sandbox,
        &["--token-counter", "tiktoken", "patch", "--help"],
    );
    let plain = run(&sandbox, &["patch", "--help"]);
    assert_eq!(counted.code, 0);
    assert!(counted.out.starts_with("usage: agentless-mcp patch"));
    assert_eq!(counted, plain);
}

// Ledger deviation: the reference reads stdin by the locale, as a traceback or as surrogate
// escapes; this build refuses bytes that are not UTF-8, as it refuses such a -f file.
#[test]
fn a_patch_on_stdin_that_is_not_utf8_is_refused() {
    let sandbox = Sandbox::new();
    let args = [OsString::from("patch"), OsString::from("parse")];
    let mut stdin = &b"### a.py\n<<<<<<< SEARCH\n\xff\n"[..];
    let got = sandbox.run_with_stdin(&args, sandbox.path(), None, &mut stdin);
    let message = "agentless-mcp: cannot read stdin: not valid UTF-8: byte 24 is invalid start \
                   byte\n";
    assert_eq!(got, refused(message));
}

struct Unreadable;

impl Read for Unreadable {
    fn read(&mut self, _bytes: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::from_raw_os_error(5))
    }
}

// Ledger deviation: the reference ends a failed stdin read with a traceback.
#[test]
fn a_stdin_that_cannot_be_read_is_refused() {
    let sandbox = Sandbox::new();
    let args = [OsString::from("patch"), OsString::from("parse")];
    let got = sandbox.run_with_stdin(&args, sandbox.path(), None, &mut Unreadable);
    let message = "agentless-mcp: cannot read stdin: Input/output error\n";
    assert_eq!(got, refused(message));
}

// The reference keeps the bytes of stdin: a "\r\n" stays in the edit, where a -f file reads it
// as "\n".
#[test]
fn a_patch_on_stdin_keeps_its_carriage_returns() {
    let sandbox = Sandbox::new();
    let text = "### a.py\n<<<<<<< SEARCH\nx\r\n=======\ny\r\n>>>>>>> REPLACE\n";
    let file = sandbox.write("patch.txt", text);
    let parse = |extra: &[OsString], stdin: &str| {
        let mut args = vec![OsString::from("patch"), OsString::from("parse")];
        args.extend_from_slice(extra);
        let got = sandbox.run_with_stdin(&args, sandbox.path(), None, &mut stdin.as_bytes());
        assert_eq!(got.code, 0, "{got:?}");
        let document: serde_json::Value = serde_json::from_str(&got.out).expect("a JSON answer");
        document["edits"][0]["search"].clone()
    };
    assert_eq!(parse(&[], text), "x\r");
    let from_file = [OsString::from("-f"), file.into_os_string()];
    assert_eq!(parse(&from_file, ""), "x");
}

// Ledger deviation: the reference hands a command that is not UTF-8 to the child as bytes.
#[test]
fn a_test_command_that_is_not_utf8_is_refused() {
    let sandbox = Sandbox::new();
    let root = sandbox.path().join("repo");
    sandbox.write("repo/app.py", "x = 1\n");
    sandbox.git(sandbox.path(), &["init", "--quiet", "-b", "main", "repo"]);
    sandbox.git(&root, &["add", "-A"]);
    sandbox.git(&root, &["commit", "--quiet", "-m", "fixture"]);
    for flag in ["--test-cmd", "--repro-cmd"] {
        let mut args: Vec<OsString> = ["validate", "--candidates", "c", "--test-cmd", "true"]
            .into_iter()
            .map(OsString::from)
            .collect();
        args.extend([
            OsString::from(flag),
            OsString::from_vec(b"run\xff".to_vec()),
        ]);
        let got = sandbox.run(&args, &root, None);
        let message = format!("agentless-mcp: {flag} is not valid UTF-8: run\u{fffd}\n");
        assert_eq!(got, refused(&message), "{flag}");
    }
}

#[test]
fn a_path_that_is_not_utf8_is_refused() {
    let sandbox = Sandbox::new();
    let path = OsString::from_vec(b"a\xff.py".to_vec());
    let args = [OsString::from("lint"), OsString::from("--path"), path];
    let args = [&args[..], &[OsString::from("--after"), OsString::from("x")]].concat();
    let got = sandbox.run(&args, sandbox.path(), None);
    let message = "agentless-mcp: --path is not valid UTF-8: a\u{fffd}.py\n";
    assert_eq!(got, refused(message));
}

struct Closed;

impl Write for Closed {
    fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
        Err(io::Error::from_raw_os_error(32))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_closed_stdout_exits_120_with_two_lines_after_the_rest_of_stderr() {
    let (mut out, mut err) = (Closed, Vec::new());
    let mut streams = Streams {
        out: &mut out,
        err: &mut err,
    };
    let code = cli::write_answer(&mut streams, "answer\n", "note\n", 3);
    assert_eq!(code, 120);
    let expected = "note\nException ignored on flushing sys.stdout:\nBrokenPipeError: [Errno 32] \
                    Broken pipe\n";
    assert_eq!(String::from_utf8(err).expect("UTF-8"), expected);
}

#[test]
fn a_hook_command_line_other_than_erosion_is_refused() {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut streams = Streams {
        out: &mut out,
        err: &mut err,
    };
    assert_eq!(cli::refuse_hook(&mut streams), 2);
    assert!(out.is_empty());
    let err = String::from_utf8(err).expect("UTF-8");
    assert_eq!(err, "agentless-mcp: hook takes one argument: erosion\n");
}

#[test]
fn a_panic_is_reported_on_one_line() {
    let seen = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&seen);
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        *sink.lock().expect("an unpoisoned line") = cli::internal_error_line(info);
    }));
    let caught = panic::catch_unwind(|| panic!("first\nsecond"));
    panic::set_hook(previous);
    assert!(caught.is_err());
    let line = seen.lock().expect("an unpoisoned line").clone();
    assert!(
        line.starts_with("agentless-mcp: internal error: first\\nsecond ("),
        "{line}"
    );
    assert!(
        line.ends_with(")\n") && line.contains("refusals.rs:"),
        "{line}"
    );
    assert_eq!(line.matches('\n').count(), 1, "{line}");
}

#[test]
fn an_unreadable_start_directory_is_reported_on_one_line() {
    let line = cli::cwd_failure_line(&io::Error::from_raw_os_error(2));
    let expected = "agentless-mcp: cannot read the current directory: FileNotFoundError(2, 'No \
                    such file or directory')\n";
    assert_eq!(line, expected);
}

// Every file below `root` outside .git, with its bytes.
fn contents(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).expect("a readable directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() && !path.ends_with(".git") {
                pending.push(path);
            } else if path.is_file() {
                found.push((path.display().to_string(), fs::read(&path).expect("a file")));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn every_command_but_an_in_place_apply_leaves_the_repository_as_it_found_it() {
    let sandbox = Sandbox::new();
    let root = sandbox.path().join("repo");
    sandbox.write("repo/app.py", "def f(x):\n    return x\n");
    sandbox.git(sandbox.path(), &["init", "--quiet", "-b", "main", "repo"]);
    sandbox.git(&root, &["add", "-A"]);
    sandbox.git(&root, &["commit", "--quiet", "-m", "fixture"]);
    sandbox.write(
        "repo/app.py",
        "def f(x):\n    if x:\n        return 1\n    return x\n",
    );
    let after = sandbox.write("after.py", "def g():\n    return 2\n");
    let patch = sandbox.write(
        "patch.txt",
        "### app.py\n<<<<<<< SEARCH\n    return x\n=======\n    return x + 1\n>>>>>>> REPLACE\n",
    );
    let state = || {
        let status = sandbox.git(&root, &["status", "--porcelain", "--untracked-files=all"]);
        (
            status,
            sandbox.git(&root, &["rev-parse", "HEAD"]),
            contents(&root),
        )
    };
    let before = state();
    let after = after.display().to_string();
    let patch = patch.display().to_string();
    let commands = [
        vec![
            "lint",
            "--since",
            "HEAD",
            "--only",
            "complexity_delta,contract_drift,clone_delta",
        ],
        vec!["lint", "--path", "app.py", "--after", &after, "--json"],
        vec!["lint", "--candidates", &patch],
        vec!["capabilities"],
        vec!["patch", "check", "-f", &patch],
        vec!["patch", "normalize", "-f", &patch],
        vec!["patch", "apply", "-f", &patch],
    ];
    for args in commands {
        let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
        assert_eq!(sandbox.run(&args, &root, None).code, 0, "{args:?}");
    }
    assert!(state() == before, "a command changed the repository");
}
