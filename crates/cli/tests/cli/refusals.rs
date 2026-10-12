//! The answers that only this build gives: the commands it does not run, the arguments it does
//! not take, the closed stdout, the panic line, and a repository that every command leaves as
//! it found it.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStringExt;
use std::panic;
use std::path::Path;
use std::sync::{Arc, Mutex};

use cli::Streams;

use crate::common::{Outcome, Sandbox};

const MAP_REFUSAL: &str =
    "agentless-mcp: this build does not run map yet. Run it with the Python agentless-mcp.\n";

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

#[test]
fn a_command_this_build_does_not_run_is_refused_before_its_arguments_are_read() {
    let sandbox = Sandbox::new();
    for args in [
        &["map"][..],
        &["map", "--help"],
        &["--bogus", "map", "--bogus"],
    ] {
        assert_eq!(run(&sandbox, args), refused(MAP_REFUSAL), "{args:?}");
    }
}

#[test]
fn the_token_counter_refusal_comes_before_the_command_refusal() {
    let got = run(
        &Sandbox::new(),
        &["--token-counter", "tiktoken", "map", "--help"],
    );
    assert_eq!(got.code, 2);
    assert!(
        got.err
            .starts_with("agentless-mcp: --token-counter tiktoken needs"),
        "{got:?}"
    );
}

#[test]
fn a_lint_source_this_build_does_not_read_is_refused() {
    let sandbox = Sandbox::new();
    for flag in ["--candidates", "--diff"] {
        let message = format!(
            "agentless-mcp: this build does not run lint {flag} yet. Use --path with --after, or \
             --since, or run the Python agentless-mcp.\n"
        );
        assert_eq!(
            run(&sandbox, &["lint", flag, "x", "--only", "arity"]),
            refused(&message)
        );
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
fn lint_and_capabilities_leave_the_repository_as_they_found_it() {
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
    let commands = [
        vec![
            "lint",
            "--since",
            "HEAD",
            "--only",
            "complexity_delta,contract_drift,clone_delta",
        ],
        vec!["lint", "--path", "app.py", "--after", &after, "--json"],
        vec!["capabilities"],
    ];
    for args in commands {
        let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
        assert_eq!(sandbox.run(&args, &root, None).code, 0, "{args:?}");
    }
    assert!(state() == before, "a command changed the repository");
}
