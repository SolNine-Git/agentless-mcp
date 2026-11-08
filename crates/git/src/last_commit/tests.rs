use std::fs;
use std::path::Path;
use std::time::Duration;

use super::{LAST_COMMIT, LastCommit, answer};
use crate::history::{FORMAT, MAX_HISTORY_OUTPUT_BYTES};
use crate::run::classify;
use crate::testing::{Scratch, path_before};
use crate::{Commit, Failure, Git, GitOutcome, Unfinished};

// A bound that a git in a small temporary repository never reaches.
const TIMEOUT: Duration = Duration::from_secs(30);
// The author date that the setup identity pins, as %aI prints it.
const AUTHORED: &str = "2026-01-01T00:00:00Z";

fn commit(scratch: &Scratch, root: &Path, revision: &str, subject: &str) -> LastCommit {
    let printed = scratch.run_git(root, &["rev-parse", revision]);
    let sha = String::from_utf8(printed).expect("a hex sha");
    LastCommit::Found(Commit {
        sha: sha.trim_end().to_owned(),
        authored: AUTHORED.to_owned(),
        subject: subject.to_owned(),
        body: String::new(),
    })
}

fn exited(detail: &str) -> GitOutcome {
    GitOutcome::Failed(Failure {
        subcommand: "log",
        code: 128,
        detail: detail.to_owned(),
    })
}

fn with_fake_git(scratch: &Scratch, body: &str) -> Git {
    let bin = scratch.fake_git(body, 0o755);
    Git::new(scratch.env_with(&[("PATH", path_before(bin).as_os_str())]))
}

#[test]
fn each_file_gives_the_newest_commit_that_touched_it() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a.py", "a = 1\n")]);
    scratch.write("repo/b.py", "b = 1\n");
    scratch.commit_all(&root, "add b");
    let git = scratch.git();
    let expected = commit(&scratch, &root, "HEAD~1", "fixture");
    assert_eq!(git.last_commit(&root, "a.py", TIMEOUT), expected);
    let expected = commit(&scratch, &root, "HEAD", "add b");
    assert_eq!(git.last_commit(&root, "b.py", TIMEOUT), expected);
}

#[test]
fn a_path_reads_below_a_root_that_is_below_the_repository_top() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("sub/a.py", "a = 1\n"), ("a.py", "a = 2\n")]);
    scratch.write("repo/a.py", "a = 3\n");
    scratch.commit_all(&root, "change the top a.py");
    let sub = root.join("sub");
    let found = scratch.git().last_commit(&sub, "a.py", TIMEOUT);
    assert_eq!(found, commit(&scratch, &root, "HEAD~1", "fixture"));
}

#[test]
fn a_glob_character_in_the_path_matches_only_itself() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a*.py", "a = 1\n")]);
    scratch.write("repo/ab.py", "b = 1\n");
    scratch.commit_all(&root, "add ab");
    let found = scratch.git().last_commit(&root, "a*.py", TIMEOUT);
    assert_eq!(found, commit(&scratch, &root, "HEAD~1", "fixture"));
}

#[test]
fn a_path_that_no_commit_touched_has_no_commit() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a.py", "a = 1\n")]);
    scratch.write("repo/new.py", "n = 1\n");
    let git = scratch.git();
    for path in ["new.py", "gone.py"] {
        let found = git.last_commit(&root, path, TIMEOUT);
        assert_eq!(found, LastCommit::NoCommit, "{path}");
    }
}

#[test]
fn a_branch_with_no_commit_has_no_commit_for_any_path() {
    let scratch = Scratch::new();
    let root = scratch.init("repo");
    scratch.write("repo/a.py", "a = 1\n");
    let found = scratch.git().last_commit(&root, "a.py", TIMEOUT);
    assert_eq!(found, LastCommit::NoCommit);
}

#[test]
fn a_folder_outside_every_repository_is_not_a_repository() {
    let scratch = Scratch::new();
    let outside = scratch.create_dir("plain");
    scratch.write("plain/a.py", "a = 1\n");
    let found = scratch.git().last_commit(&outside, "a.py", TIMEOUT);
    assert_eq!(found, LastCommit::NotRepository);
}

// The deadline has passed when the runner first waits, so no clock decides the outcome.
#[test]
fn a_git_that_outlives_the_timeout_is_timed_out() {
    let scratch = Scratch::new();
    let git = with_fake_git(&scratch, "#!/bin/sh\nexec sleep 60\n");
    let found = git.last_commit(&scratch.root, "a.py", Duration::ZERO);
    assert_eq!(found, LastCommit::TimedOut);
}

#[test]
fn the_call_asks_for_one_commit_in_the_history_format_on_the_literal_path() {
    let scratch = Scratch::new();
    let bin = scratch.fake_git("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_LOG\"\n", 0o755);
    let log = scratch.root.join("argv.log");
    let env = [("PATH", bin.as_os_str()), ("ARGV_LOG", log.as_os_str())];
    let git = Git::new(scratch.env_with(&env));
    let found = git.last_commit(&scratch.root, ":(exclude)a.py", TIMEOUT);
    assert_eq!(found, LastCommit::NoCommit);
    let argv = fs::read_to_string(&log).expect("the fake git wrote its argv");
    let lines: Vec<&str> = argv.lines().collect();
    let directory = scratch.root.to_str().expect("a UTF-8 scratch path");
    let wanted = [
        "-C",
        directory,
        "log",
        "--max-count=1",
        "-z",
        FORMAT,
        "--",
        ":(literal):(exclude)a.py",
    ];
    assert!(lines.ends_with(&wanted), "{lines:?}");
}

#[test]
fn every_other_ending_is_a_failure_with_its_note() {
    let cases = [
        (
            exited("fatal: bad object HEAD"),
            "git log exited 128: fatal: bad object HEAD".to_owned(),
        ),
        (
            GitOutcome::Unfinished(Unfinished::NotInstalled),
            "git is not installed, so repository state is unknown".to_owned(),
        ),
        (
            GitOutcome::Capped(Vec::new()),
            format!("git log printed more than {MAX_HISTORY_OUTPUT_BYTES} bytes for one commit"),
        ),
        (
            GitOutcome::Printed(b"not a sha\0".to_vec()),
            "1 fields is not a whole number of 4-field commits".to_owned(),
        ),
    ];
    for (outcome, note) in cases {
        assert_eq!(answer(outcome), LastCommit::Failed(note));
    }
}

#[test]
fn a_timeout_a_missing_repository_and_an_unborn_branch_have_their_own_outcomes() {
    let timed_out = classify("log", LAST_COMMIT, io::Outcome::TimedOut);
    assert_eq!(answer(timed_out), LastCommit::TimedOut);
    let mount = "fatal: not a git repository (or any parent up to mount point /var/tmp)";
    assert_eq!(answer(exited(mount)), LastCommit::NotRepository);
    let unborn = "fatal: your current branch 'main' does not have any commits yet";
    assert_eq!(answer(exited(unborn)), LastCommit::NoCommit);
}
