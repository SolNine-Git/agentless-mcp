//! One caller-supplied test command under a hard time bound, in its own process group, with the
//! tail of its output.
//!
//! The command is split into an argv and never run by a shell. stdout and stderr go to two
//! unlinked temporary files, not to pipes, so a chatty command never blocks on a full pipe, and
//! the files are trimmed while the command runs. A command still running at its deadline, at a
//! SIGINT, or after a failed wait has its whole process group killed: SIGTERM, a grace period,
//! SIGKILL, and a short reap.

use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{Pid, Signal, kill_process_group};
use serde::Serialize;
use text::{CodePointString, split_shell_words};
use wait_timeout::ChildExt;

use crate::interrupt::{self, Hold};
use crate::random::create_named;
use crate::{os_error_line, remove_file, strerror};

/// The default cap on the bytes kept from each stream.
pub const DEFAULT_MAX_CAPTURE: usize = 100_000;
/// How many times the cap a capture file may hold before the runner empties it.
pub const CAPTURE_SLACK: u64 = 8;
/// How often the runner looks at the child and its capture files.
pub const CAPTURE_POLL: Duration = Duration::from_millis(250);
/// The time between SIGTERM and SIGKILL to the process group.
pub const TERM_GRACE: Duration = Duration::from_secs(5);
/// The wait for the reap after SIGKILL.
pub const KILL_REAP: Duration = Duration::from_secs(1);
/// The variables of this process that a test command inherits when they are set.
pub const TEST_ENV_ALLOWLIST: [&str; 4] = ["PATH", "HOME", "LANG", "TMPDIR"];

const TEMPORARY_DIR: &str = "/tmp";
const CAPTURE_PREFIX: &str = "tmp";

/// One test command and its bounds.
#[derive(Clone, Copy, Debug)]
pub struct TestCommand<'a> {
    /// The working directory of the command.
    pub cwd: &'a Path,
    /// The command line, split into words as a POSIX shell splits them.
    pub command: &'a str,
    /// The time from the spawn until the runner kills the process group.
    pub timeout: Duration,
    /// The most bytes kept from each stream: the tail.
    pub max_capture: usize,
    /// The names of more variables that the command inherits when they are set.
    pub pass_env: &'a [String],
}

/// What became of one test command. It displays as its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// The command exited 0.
    Passed,
    /// The command exited with another status, or a signal ended it.
    Failed,
    /// The command was still running at its deadline.
    Timeout,
    /// The command could not be split or started.
    Error,
}

/// The outcome of one test command, with the tails of its output.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunResult {
    /// What became of the command.
    pub status: RunStatus,
    /// The exit status, or the negative number of the signal that ended the command. `None` for
    /// a timeout and for a command that did not start.
    pub exit_code: Option<i32>,
    /// The seconds from the start to the end of the cleanup, to 3 decimal places.
    pub duration: f64,
    /// The last bytes of stdout, decoded.
    pub stdout_tail: String,
    /// The last bytes of stderr, decoded, or the reason the command did not start.
    pub stderr_tail: String,
}

/// A finished run and the lines its group kill logged, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct TestRun {
    /// The outcome.
    pub result: RunResult,
    /// The lines of the group kill.
    pub notes: Vec<GroupNote>,
}

/// One problem of the group kill. It displays as its log line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupNote {
    /// The kernel refused the signal to the group.
    NotPermitted {
        /// The process group.
        group: u32,
    },
    /// The group leader was not reaped after SIGKILL.
    StillPresent {
        /// The process group.
        group: u32,
    },
    /// The signal to the group failed for another reason.
    SignalFailed {
        /// The process group.
        group: u32,
        /// The operating system's message.
        reason: String,
    },
}

/// Why a test run gave no result. It displays as the error text.
#[derive(Debug)]
pub enum TestRunError {
    /// SIGINT arrived. The group is killed.
    Interrupted {
        /// The lines of the group kill.
        notes: Vec<GroupNote>,
    },
    /// A capture file could not be created, trimmed or read.
    Capture {
        /// The error of the file operation.
        error: io::Error,
        /// The lines of the group kill.
        notes: Vec<GroupNote>,
    },
    /// The wait for the command failed. The group is killed.
    Wait {
        /// The error of the wait.
        error: io::Error,
        /// The lines of the group kill.
        notes: Vec<GroupNote>,
    },
    /// The SIGINT watch could not be registered. Nothing ran.
    Watch(io::Error),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    pub(crate) poll: Duration,
    pub(crate) grace: Duration,
    pub(crate) reap: Duration,
}

const BOUNDS: Bounds = Bounds {
    poll: CAPTURE_POLL,
    grace: TERM_GRACE,
    reap: KILL_REAP,
};

/// Runs `command` with the variables that `env` gives for [`TEST_ENV_ALLOWLIST`] and the
/// command's own names. A command that cannot be split or started is a result with status
/// `Error`.
///
/// # Errors
///
/// Returns [`TestRunError`] for a SIGINT, or for a failed capture file, wait or watch.
pub fn run_test(
    command: &TestCommand<'_>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<TestRun, TestRunError> {
    run_bounded(command, &env, BOUNDS)
}

pub(crate) fn run_bounded(
    command: &TestCommand<'_>,
    env: &impl Fn(&str) -> Option<OsString>,
    bounds: Bounds,
) -> Result<TestRun, TestRunError> {
    let words = match split_shell_words(command.command) {
        Ok(words) => words,
        Err(error) => {
            let reason = format!("command cannot be split into an argv: {error}");
            return Ok(not_started(reason, Duration::ZERO));
        }
    };
    let Some((program, args)) = words.split_first() else {
        return Ok(not_started("command is empty".to_owned(), Duration::ZERO));
    };
    let hold = interrupt::hold().map_err(TestRunError::Watch)?;
    let started = Instant::now();
    let captures = Captures::create(&capture_dir(env)).map_err(unkilled)?;
    let spawned = spawn(command, program, args, env, &captures).map_err(unkilled)?;
    match spawned {
        Ok(child) => {
            let running = Running {
                child,
                captures,
                hold,
                started,
            };
            running.finish(command, bounds)
        }
        Err(error) => {
            let quoted = CodePointString::from(program.as_str()).quoted();
            let reason = format!("could not start {quoted}: {}", strerror(&error));
            Ok(not_started(reason, started.elapsed()))
        }
    }
}

fn unkilled(error: io::Error) -> TestRunError {
    TestRunError::Capture {
        error,
        notes: Vec::new(),
    }
}

fn not_started(reason: String, elapsed: Duration) -> TestRun {
    TestRun {
        result: RunResult {
            status: RunStatus::Error,
            exit_code: None,
            duration: seconds(elapsed),
            stdout_tail: String::new(),
            stderr_tail: reason,
        },
        notes: Vec::new(),
    }
}

fn seconds(elapsed: Duration) -> f64 {
    json::round_half_even(elapsed.as_secs_f64(), 3)
}

// The capture files of one run. The child writes through its own copies, which share the file
// offset with these.
struct Captures {
    stdout: File,
    stderr: File,
}

impl Captures {
    fn create(directory: &Path) -> io::Result<Self> {
        Ok(Self {
            stdout: unlinked(directory)?,
            stderr: unlinked(directory)?,
        })
    }
}

fn unlinked(directory: &Path) -> io::Result<File> {
    let (path, file) = create_named(directory, CAPTURE_PREFIX.as_ref(), "")?;
    remove_file(&path)?;
    Ok(file)
}

fn capture_dir(env: &impl Fn(&str) -> Option<OsString>) -> PathBuf {
    env("TMPDIR")
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from(TEMPORARY_DIR), PathBuf::from)
}

// The outer error is a capture file that could not be handed to the child; the inner one is the
// spawn's.
fn spawn(
    command: &TestCommand<'_>,
    program: &str,
    args: &[String],
    env: &impl Fn(&str) -> Option<OsString>,
    captures: &Captures,
) -> io::Result<io::Result<Child>> {
    let stdout = captures.stdout.try_clone()?;
    let stderr = captures.stderr.try_clone()?;
    Ok(Command::new(program)
        .args(args)
        .current_dir(command.cwd)
        .env_clear()
        .envs(environment(command.pass_env, env))
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0)
        .spawn())
}

// Each allowed name and each passed name once, with its value, when `env` has it.
fn environment(
    pass_env: &[String],
    env: &impl Fn(&str) -> Option<OsString>,
) -> Vec<(String, OsString)> {
    let names = TEST_ENV_ALLOWLIST
        .into_iter()
        .chain(pass_env.iter().map(String::as_str));
    let mut kept: Vec<(String, OsString)> = Vec::new();
    for name in names {
        if kept.iter().any(|(seen, _)| seen == name) {
            continue;
        }
        if let Some(value) = env(name) {
            kept.push((name.to_owned(), value));
        }
    }
    kept
}

struct Running {
    child: Child,
    captures: Captures,
    hold: Hold,
    started: Instant,
}

enum Waited {
    Exited(ExitStatus),
    TimedOut,
    Interrupted,
    Trim(io::Error),
    Wait(io::Error),
}

impl Running {
    fn finish(
        mut self,
        command: &TestCommand<'_>,
        bounds: Bounds,
    ) -> Result<TestRun, TestRunError> {
        let waited = self.wait(command, bounds);
        // Every way out of the wait but an exit leaves the child unreaped, so the group is killed.
        let notes = match waited {
            Waited::Exited(_) => Vec::new(),
            _ => end_group(&mut self.child, bounds),
        };
        let exit = match waited {
            Waited::Exited(status) => Some(status),
            Waited::TimedOut => None,
            Waited::Interrupted => return Err(TestRunError::Interrupted { notes }),
            Waited::Trim(error) => return Err(TestRunError::Capture { error, notes }),
            Waited::Wait(error) => return Err(TestRunError::Wait { error, notes }),
        };
        let duration = seconds(self.started.elapsed());
        match self.result(exit, duration, command.max_capture) {
            Ok(result) => Ok(TestRun { result, notes }),
            Err(error) => Err(TestRunError::Capture { error, notes }),
        }
    }

    fn wait(&mut self, command: &TestCommand<'_>, bounds: Bounds) -> Waited {
        let ceiling = u64::try_from(command.max_capture.max(1))
            .unwrap_or(u64::MAX)
            .saturating_mul(CAPTURE_SLACK);
        let mut holes = [0; 2];
        let deadline = Instant::now().checked_add(command.timeout);
        loop {
            let remaining = deadline.map_or(Duration::MAX, |at| {
                at.saturating_duration_since(Instant::now())
            });
            if remaining.is_zero() {
                return Waited::TimedOut;
            }
            if self.hold.interrupted() {
                return Waited::Interrupted;
            }
            match self.child.wait_timeout(bounds.poll.min(remaining)) {
                Ok(Some(status)) => return Waited::Exited(status),
                Ok(None) => {}
                Err(error) => return Waited::Wait(error),
            }
            if let Err(error) = self.trim_all(ceiling, &mut holes) {
                return Waited::Trim(error);
            }
        }
    }

    fn trim_all(&self, ceiling: u64, holes: &mut [u64; 2]) -> io::Result<()> {
        let files = [&self.captures.stdout, &self.captures.stderr];
        for (file, hole) in files.into_iter().zip(holes.iter_mut()) {
            *hole = trim(file, ceiling, *hole)?;
        }
        Ok(())
    }

    fn result(
        &mut self,
        exit: Option<ExitStatus>,
        duration: f64,
        cap: usize,
    ) -> io::Result<RunResult> {
        let stdout_tail = tail(&mut self.captures.stdout, cap)?;
        let stderr_tail = tail(&mut self.captures.stderr, cap)?;
        let (status, exit_code) = match exit {
            None => (RunStatus::Timeout, None),
            Some(status) => {
                let code = exit_code(status);
                let passed = if code == 0 {
                    RunStatus::Passed
                } else {
                    RunStatus::Failed
                };
                (passed, Some(code))
            }
        };
        Ok(RunResult {
            status,
            exit_code,
            duration,
            stdout_tail,
            stderr_tail,
        })
    }
}

// A signal shows as its negative number. After a wait, a status is either an exit or a signal.
fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| -signal))
        .unwrap_or_else(|| status.into_raw())
}

// Empties a capture file once its live bytes pass `ceiling`, and returns the new hole. The
// child's next write lands past the end, so the size counts the hole of the previous trim.
pub(crate) fn trim(file: &File, ceiling: u64, hole: u64) -> io::Result<u64> {
    let size = file.metadata()?.len();
    if size.saturating_sub(hole) <= ceiling {
        return Ok(hole);
    }
    file.set_len(0)?;
    Ok(size)
}

// SIGTERM, the grace, SIGKILL even when the leader exited, then the reap. A leader that exits
// says nothing about the children it started.
fn end_group(child: &mut Child, bounds: Bounds) -> Vec<GroupNote> {
    let group = child.id();
    let terminated = signal_group(group, Signal::TERM);
    let _ = reaped(child, bounds.grace);
    let killed = signal_group(group, Signal::KILL);
    kill_notes(group, terminated, killed, reaped(child, bounds.reap))
}

// A failed wait means the child is gone from this process's view, so it counts as reaped.
fn reaped(child: &mut Child, within: Duration) -> bool {
    !matches!(child.wait_timeout(within), Ok(None))
}

fn signal_group(group: u32, signal: Signal) -> Result<(), Errno> {
    // The group of pid 1 is never ours, and a kill of group 1 is a kill of every process.
    match i32::try_from(group).ok().and_then(Pid::from_raw) {
        Some(pid) if pid != Pid::INIT => kill_process_group(pid, signal),
        _ => Err(Errno::SRCH),
    }
}

// The lines of the group kill. A group that has already exited is no problem.
pub(crate) fn kill_notes(
    group: u32,
    terminated: Result<(), Errno>,
    killed: Result<(), Errno>,
    reaped: bool,
) -> Vec<GroupNote> {
    let mut notes: Vec<GroupNote> = [terminated, killed]
        .into_iter()
        .filter_map(|sent| sent.err().and_then(|error| signal_note(group, error)))
        .collect();
    if !reaped {
        notes.push(GroupNote::StillPresent { group });
    }
    notes
}

fn signal_note(group: u32, error: Errno) -> Option<GroupNote> {
    if error == Errno::SRCH {
        return None;
    }
    if error == Errno::PERM {
        return Some(GroupNote::NotPermitted { group });
    }
    let reason = strerror(&io::Error::from_raw_os_error(error.raw_os_error()));
    Some(GroupNote::SignalFailed { group, reason })
}

// The last `limit` bytes of the file, without leading NUL bytes, decoded, and marked when bytes
// were dropped. The NUL bytes are the hole that a trim leaves.
pub(crate) fn tail(file: &mut File, limit: usize) -> io::Result<String> {
    let end = file.seek(SeekFrom::End(0))?;
    if end == 0 {
        return Ok(String::new());
    }
    let start = end.saturating_sub(u64::try_from(limit).unwrap_or(u64::MAX));
    file.seek(SeekFrom::Start(start))?;
    let mut window = Vec::new();
    file.read_to_end(&mut window)?;
    let live = window
        .iter()
        .position(|&byte| byte != 0)
        .unwrap_or(window.len());
    let text = String::from_utf8_lossy(&window[live..]);
    if start == 0 {
        return Ok(text.into_owned());
    }
    Ok(format!("[... {start} earlier bytes dropped ...]\n{text}"))
}

impl RunResult {
    /// Returns true only for a command that ran to its end and exited 0.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.status == RunStatus::Passed
    }
}

impl fmt::Display for RunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
            Self::Error => "error",
        })
    }
}

impl fmt::Display for GroupNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotPermitted { group } => write!(
                f,
                "not permitted to signal process group {group}; it may survive this run"
            ),
            Self::StillPresent { group } => write!(
                f,
                "process group {group} is still present after SIGKILL; it may be stuck in the \
                 kernel"
            ),
            Self::SignalFailed { group, reason } => {
                write!(f, "could not signal process group {group}: {reason}")
            }
        }
    }
}

impl fmt::Display for TestRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interrupted { .. } => f.write_str("the test run was interrupted by SIGINT"),
            Self::Capture { error, .. } => {
                write!(
                    f,
                    "a capture file of the test run failed: {}",
                    os_error_line(error)
                )
            }
            Self::Wait { error, .. } => {
                write!(
                    f,
                    "the wait for the test command failed: {}",
                    os_error_line(error)
                )
            }
            Self::Watch(error) => write!(f, "SIGINT cannot be watched: {}", os_error_line(error)),
        }
    }
}

impl std::error::Error for TestRunError {}

#[cfg(test)]
mod tests;
