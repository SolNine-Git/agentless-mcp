use std::ffi::{OsStr, OsString};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;

use super::{HARDENING_PREFIX, Limits, classify};
use crate::testing::{Scratch, cases, latin};
use crate::{Failure, Git, GitOutcome, Unfinished};

const LIMITS: Limits = Limits {
    seconds: 5,
    shown: "5.0",
    cap: 4_096,
};

fn os(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

// A negative code is the signal that killed the process.
fn exited(code: i32, stdout: &[u8], stderr: &[u8]) -> io::Outcome {
    let raw = if code < 0 { -code } else { code << 8 };
    io::Outcome::Exited {
        status: ExitStatus::from_raw(raw),
        stdout: stdout.to_vec(),
        stderr: stderr.to_vec(),
    }
}

fn note(outcome: &GitOutcome) -> String {
    match outcome {
        GitOutcome::Failed(failure) => failure.to_string(),
        GitOutcome::Unfinished(unfinished) => unfinished.to_string(),
        GitOutcome::Printed(_) | GitOutcome::Capped(_) => String::new(),
    }
}

#[test]
fn the_environment_drops_every_git_name_but_the_two_config_names() {
    let base = [
        ("GIT_DIR", "/somewhere"),
        ("GIT_SOMETHING_INVENTED_LATER", "1"),
        ("GIT_CONFIG_GLOBAL", "/some/config"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("PATH", "/usr/bin"),
        ("HOME", "/home/someone"),
        ("LC_ALL", "en_US.UTF-8"),
        ("GIT", "kept, no underscore"),
    ];
    let git = Git::new(base.map(|(name, value)| (OsString::from(name), OsString::from(value))));
    let expected = [
        ("GIT_CONFIG_GLOBAL", "/some/config"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("PATH", "/usr/bin"),
        ("HOME", "/home/someone"),
        ("GIT", "kept, no underscore"),
        ("LC_ALL", "C"),
    ];
    let expected = expected.map(|(name, value)| (OsString::from(name), OsString::from(value)));
    assert_eq!(git.env, expected);
}

#[test]
fn every_call_carries_the_prefix_then_the_directory_then_its_arguments() {
    let git = Git::new([(OsString::from("PATH"), OsString::from("/usr/bin"))]);
    let spec = git.spec(
        Path::new("/repo"),
        "rev-parse",
        &[OsStr::new("HEAD")],
        LIMITS,
    );

    let mut expected = os(&HARDENING_PREFIX);
    expected.extend(os(&["-C", "/repo", "rev-parse", "HEAD"]));
    assert_eq!(spec.program, "git");
    assert_eq!(spec.args, expected);
    assert_eq!(spec.cwd, Path::new("."));
    assert_eq!(spec.timeout, Duration::from_secs(5));
    assert_eq!((spec.stdout_cap, spec.stderr_cap), (4_096, 65_536));
    assert!(
        spec.env
            .contains(&(OsString::from("LC_ALL"), OsString::from("C")))
    );
}

#[test]
fn an_exit_of_zero_keeps_stdout_whole() {
    let outcome = classify("rev-parse", LIMITS, exited(0, b"abc\n", b"noise"));
    assert_eq!(outcome, GitOutcome::Printed(b"abc\n".to_vec()));
}

#[test]
fn another_exit_is_a_note_with_the_first_stderr_line() {
    let stderr = b"fatal: bad revision 'x'\nsecond\n";
    let outcome = classify("rev-parse", LIMITS, exited(128, b"partial", stderr));
    let failure = Failure {
        subcommand: "rev-parse",
        code: 128,
        detail: "fatal: bad revision 'x'".to_owned(),
    };
    assert_eq!(
        note(&outcome),
        "git rev-parse exited 128: fatal: bad revision 'x'"
    );
    assert_eq!(outcome, GitOutcome::Failed(failure));
}

#[test]
fn a_signal_shows_as_its_negative_number() {
    let outcome = classify("status", LIMITS, exited(-9, b"", b""));
    assert_eq!(note(&outcome), "git status exited -9: no detail");
}

#[test]
fn the_exit_note_matches_the_reference_cases() {
    let cases: Vec<(String, i32, String)> = cases("receipt", "exit_note");
    for (stderr, code, expected) in cases {
        let stderr = latin(&stderr);
        let outcome = classify("rev-parse", LIMITS, exited(code, b"", &stderr));
        assert_eq!(note(&outcome), expected, "stderr {stderr:?}");
    }
}

#[test]
fn a_cut_listing_is_capped_and_keeps_its_bytes() {
    let truncated = io::Outcome::Truncated {
        stdout: b"abcd".to_vec(),
    };
    let outcome = classify("log", LIMITS, truncated);
    assert_eq!(outcome, GitOutcome::Capped(b"abcd".to_vec()));
}

#[test]
fn a_deadline_names_the_call_and_its_seconds() {
    let outcome = classify("log", LIMITS, io::Outcome::TimedOut);
    assert_eq!(note(&outcome), "git log timed out after 5.0s");
}

// The note shows the deadline in its reference spelling, an int or a float, so the text and the
// seconds are two fields that must agree.
#[test]
fn every_limits_text_spells_its_seconds() {
    let every = [
        ("changes LISTING", crate::changes::LISTING),
        ("changes COMMIT", crate::changes::COMMIT),
        ("changes BLOB", crate::changes::BLOB),
        ("snapshot RECEIPT", crate::snapshot::RECEIPT),
        ("toplevel TOPLEVEL", crate::toplevel::TOPLEVEL),
        ("walk LISTING", crate::walk::LISTING),
        ("walk IGNORE", crate::walk::IGNORE),
        ("history HISTORY", crate::history::HISTORY),
        ("history PROBE", crate::history::PROBE),
    ];
    for (name, limits) in every {
        let whole = limits.shown.strip_suffix(".0").unwrap_or(limits.shown);
        assert_eq!(whole, limits.seconds.to_string(), "{name}");
    }
}

#[test]
fn a_missing_program_says_git_is_not_installed() {
    let error = std::io::Error::from_raw_os_error(2);
    let missing = io::Outcome::NotRun(io::NotRun::ProgramNotFound(error));
    let outcome = classify("log", LIMITS, missing);
    assert_eq!(outcome, GitOutcome::Unfinished(Unfinished::NotInstalled));
    assert_eq!(
        note(&outcome),
        "git is not installed, so repository state is unknown"
    );
}

#[test]
fn a_process_that_cannot_be_spawned_degrades_with_its_reason() {
    let error = std::io::Error::from_raw_os_error(24);
    let outcome = classify("log", LIMITS, io::Outcome::NotRun(io::NotRun::Spawn(error)));
    assert_eq!(
        note(&outcome),
        "git log could not be run: Too many open files"
    );
}

#[test]
fn a_lost_process_reads_as_one_that_could_not_be_run() {
    let error = std::io::Error::from_raw_os_error(32);
    let outcome = classify("log", LIMITS, io::Outcome::Lost(error));
    assert_eq!(note(&outcome), "git log could not be run: Broken pipe");
}

#[test]
fn a_git_that_is_not_on_the_path_is_not_installed() {
    let scratch = Scratch::new();
    let empty = scratch.create_dir("empty-bin");
    let git = Git::new(scratch.env_with(&[("PATH", empty.as_os_str())]));
    let outcome = git.run(&scratch.root, "log", &[], LIMITS);
    assert_eq!(outcome, GitOutcome::Unfinished(Unfinished::NotInstalled));
}

#[test]
fn a_real_nonzero_exit_carries_gits_own_first_line() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a.py", "x = 1\n")]);
    let rest = [OsStr::new("--verify"), OsStr::new("no-such-ref-zzz")];
    let outcome = scratch.git().run(&root, "rev-parse", &rest, LIMITS);
    assert!(
        note(&outcome).starts_with("git rev-parse exited 128: fatal:"),
        "{outcome:?}"
    );
}
