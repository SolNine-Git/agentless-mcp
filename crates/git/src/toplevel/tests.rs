use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use super::classify;
use crate::testing::{Scratch, path_before};
use crate::{Git, Toplevel};

const TIMEOUT: Duration = Duration::from_secs(5);

fn exited(raw: i32, stderr: &[u8]) -> io::Outcome {
    io::Outcome::Exited {
        status: ExitStatus::from_raw(raw),
        stdout: Vec::new(),
        stderr: stderr.to_vec(),
    }
}

fn with_fake_git(scratch: &Scratch, body: &str, mode: u32) -> Git {
    let bin = scratch.fake_git(body, mode);
    Git::new(scratch.env_with(&[("PATH", path_before(bin).as_os_str())]))
}

#[test]
fn a_directory_inside_a_repository_gives_the_root_as_git_printed_it() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("sub/a.py", "x = 1\n")]);
    let found = scratch.git().toplevel(&root.join("sub"), TIMEOUT);
    let printed = format!("{}\n", root.display()).into_bytes();
    assert_eq!(found, Toplevel::Found(printed));
}

#[test]
fn a_directory_outside_every_repository_is_not_a_repository() {
    let scratch = Scratch::new();
    let outside = scratch.create_dir("plain");
    assert_eq!(
        scratch.git().toplevel(&outside, TIMEOUT),
        Toplevel::NotRepository
    );
}

#[test]
fn another_failure_keeps_the_code_and_the_whole_stderr() {
    let scratch = Scratch::new();
    let body = "#!/bin/sh\necho 'first line' >&2\necho 'fatal: last line' >&2\nexit 3\n";
    let git = with_fake_git(&scratch, body, 0o755);
    let expected = Toplevel::Failed {
        code: 3,
        stderr: b"first line\nfatal: last line\n".to_vec(),
    };
    assert_eq!(git.toplevel(&scratch.root, TIMEOUT), expected);
}

#[test]
fn a_signal_shows_as_its_negative_number() {
    let failed = classify(exited(9, b""));
    let expected = Toplevel::Failed {
        code: -9,
        stderr: Vec::new(),
    };
    assert_eq!(failed, expected);
}

#[test]
fn the_repository_text_counts_only_on_a_failure() {
    let printed = classify(exited(0, b"fatal: not a git repository"));
    assert_eq!(printed, Toplevel::Found(Vec::new()));
    let failed = classify(exited(
        128 << 8,
        b"x\nfatal: not a git repository (or any parent)\n",
    ));
    assert_eq!(failed, Toplevel::NotRepository);
}

#[test]
fn stdout_over_the_cap_is_capped() {
    let scratch = Scratch::new();
    let body = "#!/bin/sh\nhead -c 70000 /dev/zero\n";
    let git = with_fake_git(&scratch, body, 0o755);
    assert_eq!(git.toplevel(&scratch.root, TIMEOUT), Toplevel::Capped);
}

#[test]
fn a_git_that_outlives_the_timeout_is_stopped() {
    let scratch = Scratch::new();
    let git = with_fake_git(&scratch, "#!/bin/sh\nexec sleep 5\n", 0o755);
    let started = Instant::now();
    let outcome = git.toplevel(&scratch.root, Duration::from_millis(100));
    assert_eq!(outcome, Toplevel::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_git_that_is_not_on_the_path_is_not_installed() {
    let scratch = Scratch::new();
    let empty = scratch.create_dir("empty-bin");
    let git = Git::new(scratch.env_with(&[("PATH", empty.as_os_str())]));
    assert_eq!(git.toplevel(&scratch.root, TIMEOUT), Toplevel::NotInstalled);
}

#[test]
fn a_git_that_cannot_start_gives_its_errno_and_message() {
    let scratch = Scratch::new();
    // Only the program that cannot run is on PATH, so the search does not go on to the real git.
    let bin = scratch.fake_git("#!/bin/sh\nexit 0\n", 0o644);
    let git = Git::new(scratch.env_with(&[("PATH", bin.as_os_str())]));
    let expected = Toplevel::NotRun {
        errno: Some(13),
        message: "Permission denied".to_owned(),
    };
    assert_eq!(git.toplevel(&scratch.root, TIMEOUT), expected);
}

#[test]
fn a_lost_process_gives_its_errno_and_message() {
    let lost = io::Outcome::Lost(std::io::Error::from_raw_os_error(5));
    let expected = Toplevel::NotRun {
        errno: Some(5),
        message: "Input/output error".to_owned(),
    };
    assert_eq!(classify(lost), expected);
}
