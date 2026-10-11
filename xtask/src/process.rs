use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::Failure;

pub const GIT: Duration = Duration::from_secs(30);
pub const METADATA: Duration = Duration::from_secs(120);
pub const GATE_BINARY: Duration = Duration::from_secs(300);
pub const BUILD: Duration = Duration::from_secs(1800);
const POLL: Duration = Duration::from_millis(10);

type Pipe = Receiver<std::io::Result<Vec<u8>>>;

/// A command that finished: its exit status and everything it wrote.
pub struct Finished {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run `command` with no input and captured output; kill it and fail when it outlives `deadline`.
pub fn run(command: &mut Command, deadline: Duration) -> Result<Finished, Failure> {
    let until = Instant::now() + deadline;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failure::Broken(format!("{command:?}: {error}")))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let status = wait(&mut child, until)
        .map_err(|error| Failure::Broken(format!("{command:?}: {error}")))?
        .ok_or_else(|| expired(command, deadline))?;
    Ok(Finished {
        status,
        stdout: receive(&stdout, until, command, deadline)?,
        stderr: receive(&stderr, until, command, deadline)?,
    })
}

/// Run `command` under `deadline` and return its stdout; a non-zero exit is a failure.
pub fn stdout(command: &mut Command, deadline: Duration) -> Result<Vec<u8>, Failure> {
    let finished = run(command, deadline)?;
    if finished.status.success() {
        Ok(finished.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&finished.stderr);
        Err(Failure::Broken(format!(
            "{command:?} exited with {}: {}",
            finished.status,
            stderr.trim()
        )))
    }
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> Pipe {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = match pipe {
            Some(mut pipe) => pipe.read_to_end(&mut bytes).map(|_| bytes),
            None => Ok(bytes),
        };
        // The receiver is gone only after the deadline has already failed the run.
        sender.send(read).ok();
    });
    receiver
}

fn wait(child: &mut Child, until: Instant) -> std::io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= until {
            child.kill()?;
            child.wait()?;
            return Ok(None);
        }
        thread::sleep(POLL);
    }
}

fn receive(
    pipe: &Pipe,
    until: Instant,
    command: &Command,
    deadline: Duration,
) -> Result<Vec<u8>, Failure> {
    match pipe.recv_timeout(until.saturating_duration_since(Instant::now())) {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => Err(Failure::Broken(format!(
            "{command:?}: cannot read its output: {error}"
        ))),
        Err(RecvTimeoutError::Timeout) => Err(expired(command, deadline)),
        Err(RecvTimeoutError::Disconnected) => Err(Failure::Broken(format!(
            "{command:?}: its output reader stopped"
        ))),
    }
}

fn expired(command: &Command, deadline: Duration) -> Failure {
    Failure::Broken(format!(
        "{command:?} did not finish within its deadline of {deadline:?}"
    ))
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::{Duration, Instant};

    use super::{run, stdout};
    use crate::Failure;

    const LONG: Duration = Duration::from_secs(10);

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    fn misses_a_short_deadline_quickly(script: &str) -> bool {
        let started = Instant::now();
        let outcome = run(&mut shell(script), Duration::from_millis(300));
        let quick = started.elapsed() < Duration::from_secs(3);
        quick && matches!(outcome, Err(Failure::Broken(message)) if message.contains("deadline"))
    }

    #[test]
    fn output_and_exit_status_are_captured() {
        let finished = run(&mut shell("echo out; echo err >&2; exit 3"), LONG).unwrap();
        assert_eq!(finished.status.code(), Some(3));
        assert_eq!(finished.stdout, b"out\n");
        assert_eq!(finished.stderr, b"err\n");
    }

    #[test]
    fn a_command_that_outlives_its_deadline_is_killed() {
        assert!(misses_a_short_deadline_quickly("sleep 5"));
    }

    #[test]
    fn a_pipe_held_open_after_exit_is_bounded_by_the_deadline() {
        assert!(misses_a_short_deadline_quickly("sleep 5 & echo done"));
    }

    #[test]
    fn stdout_fails_on_a_non_zero_exit_with_stderr() {
        let outcome = stdout(&mut shell("echo kept; echo why >&2; exit 1"), LONG);
        assert!(matches!(outcome, Err(Failure::Broken(message)) if message.contains("why")));
        assert_eq!(stdout(&mut shell("echo kept"), LONG).unwrap(), b"kept\n");
    }
}
