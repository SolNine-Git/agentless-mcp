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
pub const RUSTFMT: Duration = Duration::from_secs(60);
const SIGNAL: Duration = Duration::from_secs(5);
// After the child exits, its readers only need to be scheduled to reach the end of each pipe. A
// process that holds a pipe past this is a leak, whatever deadline remains.
const GRACE: Duration = Duration::from_secs(2);
// std has no wait with a timeout, so `wait` polls.
const POLL: Duration = Duration::from_millis(10);
// A command can carry hundreds of flags, so a message shows this many words: the program and its
// first arguments.
const LABEL_WORDS: usize = 25;

type Pipe = Receiver<std::io::Result<Vec<u8>>>;

/// Run `command` with no input and captured output; kill its process group and fail when it outlives `deadline`.
pub fn run(command: &mut Command, deadline: Duration) -> Result<Output, Failure> {
    let until = Instant::now() + deadline;
    // Its own group lets the deadline kill its descendants too. A terminal Ctrl-C then stops only
    // xtask: the child runs on, with no deadline, until it writes to the closed pipe or exits.
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|error| Failure::Broken(format!("{}: {error}", label(command))))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let waited = wait(&mut child, until)
        .map_err(|error| Failure::Broken(format!("{}: {error}", label(command))))?;
    let Some(status) = waited else {
        return Err(stop(&mut child, command, deadline));
    };
    let readers_until = Instant::now() + GRACE;
    let outputs =
        receive(&stdout, readers_until, command)?.zip(receive(&stderr, readers_until, command)?);
    let Some((stdout, stderr)) = outputs else {
        return Err(held(child.id(), command, status));
    };
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Return `command` for a message: its program and first arguments, and a count of the rest.
pub fn label(command: &Command) -> String {
    let words: Vec<String> = std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|word| word.display().to_string())
        .collect();
    let left_out = words.len().saturating_sub(LABEL_WORDS);
    let shown = words[..words.len() - left_out].join(" ");
    if left_out == 0 {
        shown
    } else {
        format!("{shown} and {left_out} more arguments")
    }
}

/// Run `command` under `deadline` and return its stdout; a non-zero exit is a failure.
pub fn stdout(command: &mut Command, deadline: Duration) -> Result<Vec<u8>, Failure> {
    let finished = run(command, deadline)?;
    if finished.status.success() {
        Ok(finished.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&finished.stderr);
        Err(Failure::Broken(format!(
            "{} exited with {}: {}",
            label(command),
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
        "{} did not finish within its deadline of {deadline:?}",
        label(command)
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

fn held(leader: u32, command: &Command, status: ExitStatus) -> Failure {
    let mut notes = vec![format!(
        "{} exited with {status}, but a process it started held its output open past the {GRACE:?} grace after the exit",
        label(command)
    )];
    // The leader is reaped, so its group id stays reserved only while a member lives. A holder that
    // left the group (setsid) survives, and the kernel may then reuse the empty group's id.
    if let Err(error) = kill_group(leader) {
        notes.push(format!("cannot kill its process group: {error}"));
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
        .map_err(|error| format!("{}: {error}", label(&command)))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} exited with {status}", label(&command)))
    }
}

// None: the pipe stayed open until `until`.
fn receive(pipe: &Pipe, until: Instant, command: &Command) -> Result<Option<Vec<u8>>, Failure> {
    match pipe.recv_timeout(until.saturating_duration_since(Instant::now())) {
        Ok(Ok(bytes)) => Ok(Some(bytes)),
        Ok(Err(error)) => Err(Failure::Broken(format!(
            "{}: cannot read its output: {error}",
            label(command)
        ))),
        Err(RecvTimeoutError::Timeout) => Ok(None),
        Err(RecvTimeoutError::Disconnected) => Err(Failure::Broken(format!(
            "{}: its output reader stopped",
            label(command)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{label, run, stdout};
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
    fn a_held_pipe_fails_after_the_grace_under_a_long_deadline_and_its_holder_dies() {
        let dir = std::env::temp_dir().join(format!("xtask-held-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("pid");
        let script = format!("sleep 30 & echo $! > '{}'; echo done", pid_file.display());
        let started = Instant::now();
        let outcome = run(&mut shell(&script), LONG);
        let elapsed = started.elapsed();
        let holder = std::fs::read_to_string(&pid_file).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(elapsed < Duration::from_secs(4), "waited {elapsed:?}");
        assert!(
            matches!(outcome, Err(Failure::Broken(message)) if message.contains("held its output open past the 2s grace"))
        );
        assert!(gone(holder.trim()), "holder {holder} outlived the run");
    }

    #[test]
    fn a_label_names_the_program_and_counts_the_arguments_it_leaves_out() {
        let mut command = Command::new("cargo");
        command.args((0..40).map(|index| format!("--force-warn=lint{index}")));
        let shown = label(&command);
        assert!(shown.starts_with("cargo --force-warn=lint0 "), "{shown}");
        assert!(
            shown.ends_with(" --force-warn=lint23 and 16 more arguments"),
            "{shown}"
        );
        assert_eq!(label(Command::new("git").arg("log")), "git log");
    }

    #[test]
    fn stdout_fails_on_a_non_zero_exit_with_stderr() {
        let outcome = stdout(&mut shell("echo kept; echo why >&2; exit 1"), LONG);
        assert!(matches!(outcome, Err(Failure::Broken(message)) if message.contains("why")));
        assert_eq!(stdout(&mut shell("echo kept"), LONG).unwrap(), b"kept\n");
    }
}
