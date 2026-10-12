//! One child process under a deadline and output caps, with its outcome as a value.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SendError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use wait_timeout::ChildExt;

use crate::strerror;

// The default Linux pipe capacity, so one read can empty a full pipe.
const CHUNK_BYTES: usize = 65_536;

/// What to run and the bounds it runs under. stdin is null; the child gets `env` and nothing else.
#[derive(Clone, Debug)]
pub struct Spec {
    /// The program, found through the `PATH` in `env` when it holds no slash.
    pub program: OsString,
    /// The arguments after the program name.
    pub args: Vec<OsString>,
    /// The working directory of the child.
    pub cwd: PathBuf,
    /// The complete environment of the child.
    pub env: Vec<(OsString, OsString)>,
    /// The time from the spawn until the runner kills the child. The reap after a kill has no
    /// bound.
    pub timeout: Duration,
    /// The most stdout bytes kept; one byte more kills the child.
    pub stdout_cap: usize,
    /// The most stderr bytes kept; the rest is read and dropped.
    pub stderr_cap: usize,
}

/// How a run ended.
#[derive(Debug)]
pub enum Outcome {
    /// The child exited, or a signal ended it, before the deadline.
    Exited {
        status: ExitStatus,
        /// All of stdout.
        stdout: Vec<u8>,
        /// The first `stderr_cap` bytes of stderr.
        stderr: Vec<u8>,
    },
    /// stdout passed its cap, so the runner killed the child.
    Truncated {
        /// The first `stdout_cap` bytes of stdout.
        stdout: Vec<u8>,
    },
    /// The deadline passed, so the runner killed the child.
    TimedOut,
    /// The child did not start.
    NotRun(NotRun),
    /// The child started, and then a pipe read, the wait or the kill failed: the result is unknown.
    Lost(io::Error),
}

/// Why a child did not start. It displays as the OS message.
#[derive(Debug)]
pub enum NotRun {
    /// No executable was found for the program.
    ProgramNotFound(io::Error),
    /// The spawn failed for another reason, a missing working directory included.
    Spawn(io::Error),
}

impl fmt::Display for NotRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Self::ProgramNotFound(error) | Self::Spawn(error)) = self;
        f.write_str(&strerror(error))
    }
}

/// Runs `spec.program` once, kills it at the deadline or when stdout passes its cap, and reaps it.
#[must_use]
pub fn run(spec: &Spec) -> Outcome {
    let mut child = match spawn(spec) {
        Ok(child) => child,
        Err(error) => return Outcome::NotRun(not_run(error, spec)),
    };
    let deadline = Instant::now().checked_add(spec.timeout);
    match drain(&mut child, spec, deadline) {
        Drained::Done { stdout, stderr } => reap(&mut child, stdout, stderr, deadline),
        Drained::Truncated(stdout) => stopped(&mut child, Outcome::Truncated { stdout }),
        Drained::TimedOut => stopped(&mut child, Outcome::TimedOut),
        Drained::Failed(error) => stopped(&mut child, Outcome::Lost(error)),
    }
}

// A full path spares the fork that a PATH search in the child costs. When that start fails, the
// child's own search runs, so its outcome and error stay what the search alone gives.
fn spawn(spec: &Spec) -> io::Result<Child> {
    if let Some(found) = found_on_path(spec, probe) {
        let mut command = command(spec, found.as_os_str());
        command.arg0(&spec.program);
        if let Ok(child) = command.spawn() {
            return Ok(child);
        }
    }
    command(spec, &spec.program).spawn()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Candidate {
    Absent,
    Runnable,
    Unknown,
}

// The file that execvp would start for a bare program name, when a stat can tell. None leaves the
// search to execvp: no PATH, a relative entry first, or a candidate that a stat cannot judge.
fn found_on_path(spec: &Spec, probe: impl Fn(&Path) -> Candidate) -> Option<PathBuf> {
    let program = spec.program.as_os_str();
    if program.is_empty() || program.as_bytes().contains(&b'/') {
        return None;
    }
    let (_, path) = spec.env.iter().rev().find(|(name, _)| name == "PATH")?;
    for directory in env::split_paths(path) {
        if !directory.is_absolute() {
            return None;
        }
        let candidate = directory.join(program);
        match probe(&candidate) {
            Candidate::Absent => {}
            Candidate::Runnable => return Some(candidate),
            Candidate::Unknown => return None,
        }
    }
    None
}

// execvp passes over a missing file, as here. All three execute bits let every user start a file.
fn probe(candidate: &Path) -> Candidate {
    match fs::metadata(candidate) {
        Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o111 == 0o111 => {
            Candidate::Runnable
        }
        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            Candidate::Absent
        }
        _ => Candidate::Unknown,
    }
}

fn command(spec: &Spec, program: &OsStr) -> Command {
    let mut command = Command::new(program);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(spec.env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn not_run(error: io::Error, spec: &Spec) -> NotRun {
    // A missing working directory fails with ENOENT too, so it does not count as a missing program.
    if error.kind() == io::ErrorKind::NotFound && spec.cwd.is_dir() {
        NotRun::ProgramNotFound(error)
    } else {
        NotRun::Spawn(error)
    }
}

enum Drained {
    Done { stdout: Vec<u8>, stderr: Vec<u8> },
    Truncated(Vec<u8>),
    TimedOut,
    Failed(io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stream {
    Stdout = 0,
    Stderr = 1,
}

struct Capped {
    bytes: Vec<u8>,
    over: bool,
}

struct Event {
    stream: Stream,
    result: io::Result<Capped>,
}

type Pipe = Box<dyn Read + Send>;

// Each pipe has its own reader thread, so a full stderr pipe cannot stall the stdout read.
fn drain(child: &mut Child, spec: &Spec, deadline: Option<Instant>) -> Drained {
    let (sender, events) = mpsc::channel();
    let readers = [
        (
            Stream::Stdout,
            child.stdout.take().map(box_pipe),
            spec.stdout_cap,
        ),
        (
            Stream::Stderr,
            child.stderr.take().map(box_pipe),
            spec.stderr_cap,
        ),
    ];
    for (stream, pipe, cap) in readers {
        let Some(pipe) = pipe else {
            return Drained::Failed(io::Error::other(format!(
                "the child has no {stream:?} pipe"
            )));
        };
        if let Err(error) = spawn_reader(stream, pipe, cap, sender.clone()) {
            return Drained::Failed(error);
        }
    }
    drop(sender);
    collect(&events, deadline)
}

fn box_pipe(pipe: impl Read + Send + 'static) -> Pipe {
    Box::new(pipe)
}

fn spawn_reader(stream: Stream, pipe: Pipe, cap: usize, sender: Sender<Event>) -> io::Result<()> {
    let name = format!("io-run-{stream:?}");
    thread::Builder::new().name(name).spawn(move || {
        let result = read_capped(pipe, cap, stream == Stream::Stdout);
        if let Err(SendError(late)) = sender.send(Event { stream, result }) {
            // The runner returned after a kill, so nothing waits for this result.
            drop(late);
        }
    })?;
    Ok(())
}

// Reads to end of file and keeps the first `cap` bytes; with `stop_at_cap` it stops at one more.
fn read_capped(mut pipe: Pipe, cap: usize, stop_at_cap: bool) -> io::Result<Capped> {
    let mut kept = Vec::new();
    let mut over = false;
    let mut chunk = vec![0; CHUNK_BYTES];
    loop {
        let count = match pipe.read(&mut chunk) {
            Ok(0) => return Ok(Capped { bytes: kept, over }),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let room = cap - kept.len();
        kept.extend_from_slice(&chunk[..count.min(room)]);
        over |= count > room;
        if over && stop_at_cap {
            return Ok(Capped { bytes: kept, over });
        }
    }
}

fn collect(events: &Receiver<Event>, deadline: Option<Instant>) -> Drained {
    let mut done: [Option<Vec<u8>>; 2] = [None, None];
    loop {
        if let [Some(stdout), Some(stderr)] = &mut done {
            return Drained::Done {
                stdout: std::mem::take(stdout),
                stderr: std::mem::take(stderr),
            };
        }
        let wait = remaining(deadline);
        if wait.is_zero() {
            return Drained::TimedOut;
        }
        match events.recv_timeout(wait) {
            Ok(Event {
                result: Err(error), ..
            }) => return Drained::Failed(error),
            Ok(Event {
                stream: Stream::Stdout,
                result: Ok(Capped { bytes, over: true }),
            }) => return Drained::Truncated(bytes),
            Ok(Event {
                stream,
                result: Ok(capped),
            }) => done[stream as usize] = Some(capped.bytes),
            Err(RecvTimeoutError::Timeout) => return Drained::TimedOut,
            Err(RecvTimeoutError::Disconnected) => {
                return Drained::Failed(io::Error::other("a pipe reader stopped without a result"));
            }
        }
    }
}

fn reap(child: &mut Child, stdout: Vec<u8>, stderr: Vec<u8>, deadline: Option<Instant>) -> Outcome {
    // Both pipes are at end of file, so the child is on its way out; the wait is still bounded.
    match child.wait_timeout(remaining(deadline)) {
        Ok(Some(status)) => Outcome::Exited {
            status,
            stdout,
            stderr,
        },
        Ok(None) => stopped(child, Outcome::TimedOut),
        Err(error) => stopped(child, Outcome::Lost(error)),
    }
}

// Kills and reaps the child. `outcome` stands unless the kill or the reap fails.
fn stopped(child: &mut Child, outcome: Outcome) -> Outcome {
    match child.kill().and_then(|()| child.wait()) {
        Ok(_) => outcome,
        Err(error) => Outcome::Lost(error),
    }
}

fn remaining(deadline: Option<Instant>) -> Duration {
    deadline.map_or(Duration::MAX, |at| {
        at.saturating_duration_since(Instant::now())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::tree;
    use crate::testing::Tree;
    use std::os::unix::process::ExitStatusExt;

    const LONG: Duration = Duration::from_secs(20);

    fn sh(script: &str, timeout: Duration) -> Spec {
        Spec {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: "/".into(),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            timeout,
            stdout_cap: 1024,
            stderr_cap: 1024,
        }
    }

    fn exited(outcome: Outcome) -> (ExitStatus, Vec<u8>, Vec<u8>) {
        match outcome {
            Outcome::Exited {
                status,
                stdout,
                stderr,
            } => (status, stdout, stderr),
            other => panic!("expected an exit, got {other:?}"),
        }
    }

    #[test]
    fn an_exit_keeps_both_streams_and_the_status() {
        let (status, stdout, stderr) = exited(run(&sh("printf out; printf err >&2; exit 3", LONG)));
        assert_eq!(
            (status.code(), &stdout[..], &stderr[..]),
            (Some(3), &b"out"[..], &b"err"[..])
        );
    }

    #[test]
    fn stderr_past_its_cap_is_cut_and_the_child_still_exits() {
        let spec = Spec {
            stderr_cap: 4,
            ..sh("printf 0123456789 >&2; printf done", LONG)
        };
        let (status, stdout, stderr) = exited(run(&spec));
        assert_eq!(
            (status.code(), &stdout[..], &stderr[..]),
            (Some(0), &b"done"[..], &b"0123"[..])
        );
    }

    #[test]
    fn stdout_at_its_cap_is_complete() {
        let spec = Spec {
            stdout_cap: 4,
            ..sh("printf 0123", LONG)
        };
        assert_eq!(exited(run(&spec)).1, b"0123");
    }

    #[test]
    fn stdout_past_its_cap_kills_the_child_and_keeps_the_cap() {
        let spec = Spec {
            stdout_cap: 4,
            ..sh("printf 01234; exec sleep 30", LONG)
        };
        let started = Instant::now();
        let outcome = run(&spec);
        assert!(started.elapsed() < LONG, "the kill came after the timeout");
        assert!(matches!(outcome, Outcome::Truncated { stdout } if stdout == b"0123"));
    }

    #[test]
    fn the_deadline_kills_a_child_that_does_not_finish() {
        let started = Instant::now();
        let outcome = run(&sh("exec sleep 30", Duration::from_millis(200)));
        assert!(started.elapsed() < LONG, "the kill came after the sleep");
        assert!(matches!(outcome, Outcome::TimedOut), "{outcome:?}");
    }

    #[test]
    fn the_deadline_also_bounds_the_reap_after_the_pipes_close() {
        let script = "exec >&- 2>&-; exec sleep 30";
        let started = Instant::now();
        let outcome = run(&sh(script, Duration::from_millis(200)));
        assert!(started.elapsed() < LONG, "the kill came after the sleep");
        assert!(matches!(outcome, Outcome::TimedOut), "{outcome:?}");
    }

    #[test]
    fn a_signal_ends_the_child_with_an_exit_outcome() {
        let (status, _, _) = exited(run(&sh("kill -9 $$", LONG)));
        assert_eq!(status.signal(), Some(9));
    }

    #[test]
    fn stdin_is_empty() {
        assert_eq!(exited(run(&sh("cat", LONG))).1, b"");
    }

    #[test]
    fn the_environment_holds_only_the_given_pairs() {
        let mut spec = sh(r#"printf %s "${HOME-unset}:$X""#, LONG);
        spec.env.push(("X".into(), "1".into()));
        assert_eq!(exited(run(&spec)).1, b"unset:1");
    }

    #[test]
    fn the_child_runs_in_the_working_directory() {
        let tree = tree();
        let spec = Spec {
            cwd: tree.path().to_path_buf(),
            ..sh("pwd", LONG)
        };
        let stdout = exited(run(&spec)).1;
        let printed = std::str::from_utf8(&stdout).expect("pwd prints the UTF-8 test path");
        assert_eq!(Path::new(printed.trim_end()), tree.path());
    }

    fn assert_not_found(spec: &Spec) {
        match run(spec) {
            Outcome::NotRun(not_run @ NotRun::ProgramNotFound(_)) => {
                assert_eq!(not_run.to_string(), "No such file or directory");
            }
            other => panic!("expected a missing program, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_program_is_not_run() {
        let spec = Spec {
            program: "/nonexistent-io-runner-test/program".into(),
            ..sh("", LONG)
        };
        assert_not_found(&spec);
    }

    #[test]
    fn a_bare_name_on_no_path_entry_is_not_found() {
        let tree = tree();
        let empty = tree
            .create_dir("empty")
            .expect("the tree takes a directory");
        let spec = Spec {
            program: "io-runner-test-absent-program".into(),
            env: vec![("PATH".into(), empty.into_os_string())],
            ..sh("", LONG)
        };
        assert_not_found(&spec);
    }

    #[test]
    fn a_missing_working_directory_is_a_spawn_failure() {
        let tree = tree();
        let spec = Spec {
            cwd: tree.join("missing"),
            ..sh("true", LONG)
        };
        let outcome = run(&spec);
        assert!(
            matches!(outcome, Outcome::NotRun(NotRun::Spawn(_))),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_bare_name_resolves_only_when_each_earlier_entry_is_absent() {
        let probe = |candidate: &Path| match candidate.parent().and_then(Path::to_str) {
            Some("/absent") => Candidate::Absent,
            Some("/runnable") => Candidate::Runnable,
            _ => Candidate::Unknown,
        };
        let found = |program: &str, paths: &[&str]| {
            let env = paths
                .iter()
                .map(|path| ("PATH".into(), path.into()))
                .collect();
            let spec = Spec {
                program: program.into(),
                env,
                ..sh("", LONG)
            };
            found_on_path(&spec, probe)
        };
        let runnable = Some(PathBuf::from("/runnable/git"));
        assert_eq!(found("git", &["/absent:/runnable:/other"]), runnable);
        assert_eq!(found("git", &["/other", "/absent:/runnable"]), runnable);
        for (program, paths) in [
            ("git", &["/absent:/other:/runnable"][..]),
            ("git", &["relative:/runnable"]),
            ("git", &[":/runnable"]),
            ("git", &["/absent"]),
            ("git", &[]),
            ("bin/git", &["/runnable"]),
            ("", &["/runnable"]),
        ] {
            assert_eq!(found(program, paths), None, "{program:?} on {paths:?}");
        }
    }

    #[test]
    fn the_probe_runs_only_a_file_that_every_user_may_start() {
        let tree = tree();
        for (name, mode) in [("all", 0o755), ("owner", 0o744), ("none", 0o644)] {
            tree.write_file(name, "").expect("the tree takes a file");
            tree.chmod(name, mode);
        }
        tree.create_dir("directory")
            .expect("the tree takes a directory");
        let probed = |name: &str| probe(&tree.join(name));
        assert_eq!(probed("all"), Candidate::Runnable);
        for name in ["owner", "none", "directory"] {
            assert_eq!(probed(name), Candidate::Unknown, "{name}");
        }
        assert_eq!(probed("missing"), Candidate::Absent);
        assert_eq!(probed("all/below"), Candidate::Absent);
    }

    #[test]
    fn a_relative_path_entry_first_starts_what_execvp_starts() {
        let tree = tree();
        placed_program(&tree, "relative", 0o755);
        placed_program(&tree, "absolute", 0o755);
        let path = [PathBuf::from("relative"), tree.join("absolute")];
        let (started, reference) = started_and_reference(&tree, &path);
        assert_eq!(started, reference);
    }

    #[test]
    fn a_program_without_the_execute_bit_earlier_starts_what_execvp_starts() {
        let tree = tree();
        placed_program(&tree, "plain", 0o644);
        placed_program(&tree, "runnable", 0o755);
        let path = [tree.join("plain"), tree.join("runnable")];
        let (started, reference) = started_and_reference(&tree, &path);
        assert_eq!(started, reference);
    }

    // The program prints the name of its directory, so the output names the file that started.
    fn placed_program(tree: &Tree, directory: &str, mode: u32) {
        let relative = format!("{directory}/prog");
        tree.create_dir(directory)
            .expect("the tree takes a directory");
        let script = format!("#!/bin/sh\nprintf {directory}\n");
        tree.write_file(&relative, script)
            .expect("the tree takes a file");
        tree.chmod(&relative, mode);
    }

    // env(1) starts the program through execvp, which makes the reference search of `path`.
    fn started_and_reference(tree: &Tree, path: &[PathBuf]) -> (Vec<u8>, Vec<u8>) {
        let path = env::join_paths(path).expect("a PATH value");
        let spec = |program: &str, args: &[&str]| Spec {
            program: program.into(),
            args: args.iter().map(OsString::from).collect(),
            cwd: tree.path().to_path_buf(),
            env: vec![("PATH".into(), path.clone())],
            ..sh("", LONG)
        };
        let started = exited(run(&spec("prog", &[]))).1;
        let reference = exited(run(&spec("/usr/bin/env", &["prog"]))).1;
        assert!(!reference.is_empty(), "the reference started no program");
        (started, reference)
    }
}
