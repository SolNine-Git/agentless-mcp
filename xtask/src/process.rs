use std::io::{ErrorKind, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::Failure;

// Hang guards, not budgets: each sits far above its command's warm run, so only a hung command
// reaches it.
pub const GIT: Duration = Duration::from_secs(30);
pub const METADATA: Duration = Duration::from_secs(120);
pub const GATE_BINARY: Duration = Duration::from_secs(300);
pub const BUILD: Duration = Duration::from_secs(1800);
const SIGNAL: Duration = Duration::from_secs(5);
// After the child exits, its readers only need to be scheduled to reach the end of each pipe.
const GRACE: Duration = Duration::from_secs(2);
// std has no wait with a timeout, so `wait` polls.
const POLL: Duration = Duration::from_millis(10);

type Pipe = Receiver<std::io::Result<Vec<u8>>>;

/// Run `command` with no input and captured output; kill its process group and fail when it outlives `deadline`.
pub fn run(command: &mut Command, deadline: Duration) -> Result<Output, Failure> {
    let until = Instant::now() + deadline;
    // Its own group lets the deadline kill its descendants too. A terminal Ctrl-C then misses it,
    // and it fails at its next write to the closed pipe or ends on its own.
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| Failure::Broken(format!("{command:?}: {error}")))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let waited = wait(&mut child, until)
        .map_err(|error| Failure::Broken(format!("{command:?}: {error}")))?;
    let Some(status) = waited else {
        return Err(stop(&mut child, command, deadline));
    };
    let held = format!(
        "{command:?} exited with {status}, but a process it started held its output open past the deadline of {deadline:?}"
    );
    let readers_until = until.max(Instant::now() + GRACE);
    Ok(Output {
        status,
        stdout: receive(&stdout, readers_until, command, &held)?,
        stderr: receive(&stderr, readers_until, command, &held)?,
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
        // A dropped receiver means run() has already failed, so these bytes have no reader.
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
            return Ok(None);
        }
        thread::sleep(POLL);
    }
}

fn stop(child: &mut Child, command: &Command, deadline: Duration) -> Failure {
    let mut notes = vec![format!(
        "{command:?} did not finish within its deadline of {deadline:?}"
    )];
    // The leader is not reaped yet, so no other process group can hold its id.
    if let Err(error) = kill_group(child.id()) {
        notes.push(format!("its process group may survive: {error}"));
    }
    if let Err(error) = child.kill().and_then(|()| child.wait()) {
        notes.push(format!("cannot kill and reap it: {error}"));
    }
    Failure::Broken(notes.join("; "))
}

fn kill_group(leader: u32) -> Result<(), String> {
    let mut command = Command::new("kill");
    command
        .args(["-s", "KILL", "--", &format!("-{leader}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let status = command
        .spawn()
        .and_then(|mut killer| {
            wait(&mut killer, Instant::now() + SIGNAL)?.map_or_else(
                || {
                    killer.kill()?;
                    killer.wait()?;
                    Err(std::io::Error::new(ErrorKind::TimedOut, "it hung"))
                },
                Ok,
            )
        })
        .map_err(|error| format!("{command:?}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{command:?} exited with {status}"))
    }
}

fn receive(pipe: &Pipe, until: Instant, command: &Command, held: &str) -> Result<Vec<u8>, Failure> {
    match pipe.recv_timeout(until.saturating_duration_since(Instant::now())) {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => Err(Failure::Broken(format!(
            "{command:?}: cannot read its output: {error}"
        ))),
        Err(RecvTimeoutError::Timeout) => Err(Failure::Broken(held.to_owned())),
        Err(RecvTimeoutError::Disconnected) => Err(Failure::Broken(format!(
            "{command:?}: its output reader stopped"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{run, stdout};
    use crate::Failure;

    const LONG: Duration = Duration::from_secs(10);
    const SHORT: Duration = Duration::from_millis(300);
    const PIPE_BUFFER_AND_MORE: usize = 1 << 20;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    fn misses_a_short_deadline_quickly(script: &str) -> bool {
        let started = Instant::now();
        let outcome = run(&mut shell(script), SHORT);
        let quick = started.elapsed() < Duration::from_secs(3);
        quick && matches!(outcome, Err(Failure::Broken(message)) if message.contains("deadline"))
    }

    fn gone(pid: &str) -> bool {
        // A killed process stays visible to `kill -0` until its new parent reaps it.
        (0..100).any(|_| {
            let alive = run(Command::new("kill").args(["-0", pid]), LONG)
                .unwrap()
                .status
                .success();
            thread::sleep(Duration::from_millis(20));
            !alive
        })
    }

    #[test]
    fn a_child_that_ignores_sigterm_is_still_killed_at_the_deadline() {
        assert!(misses_a_short_deadline_quickly("trap '' TERM; sleep 5"));
    }

    #[test]
    fn output_larger_than_the_pipe_buffer_is_read_in_full() {
        let script = format!("head -c {PIPE_BUFFER_AND_MORE} /dev/zero | tee /dev/stderr");
        let output = run(&mut shell(&script), LONG).unwrap();
        assert_eq!(output.stdout.len(), PIPE_BUFFER_AND_MORE);
        assert_eq!(output.stderr, output.stdout);
    }

    #[test]
    fn the_deadline_kills_the_whole_process_group() {
        let dir = std::env::temp_dir().join(format!("xtask-group-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("pid");
        let script = format!("sleep 30 & echo $! > '{}'; wait", pid_file.display());
        assert!(run(&mut shell(&script), SHORT).is_err());
        let grandchild = std::fs::read_to_string(&pid_file).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(
            gone(grandchild.trim()),
            "grandchild {grandchild} outlived the deadline"
        );
    }

    #[test]
    fn output_closed_within_the_grace_after_a_timely_exit_is_read_in_full() {
        let output = run(&mut shell("(sleep 0.5; echo late) & echo early"), SHORT).unwrap();
        assert_eq!(output.stdout, b"early\nlate\n");
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
    fn a_pipe_held_open_after_exit_is_reported_as_held_not_as_unfinished() {
        let started = Instant::now();
        let outcome = run(&mut shell("sleep 5 & echo done"), SHORT);
        assert!(started.elapsed() < Duration::from_secs(4));
        let Err(Failure::Broken(message)) = outcome else {
            panic!("a held pipe must fail the run");
        };
        assert!(
            message.contains(
                "exited with exit status: 0, but a process it started held its output open"
            ),
            "{message}"
        );
    }

    #[test]
    fn stdout_fails_on_a_non_zero_exit_with_stderr() {
        let outcome = stdout(&mut shell("echo kept; echo why >&2; exit 1"), LONG);
        assert!(matches!(outcome, Err(Failure::Broken(message)) if message.contains("why")));
        assert_eq!(stdout(&mut shell("echo kept"), LONG).unwrap(), b"kept\n");
    }
}
