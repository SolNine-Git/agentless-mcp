use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use super::{
    BLOB, COMMIT, Change, LISTING, Side, looks_binary, parse_changes, parse_commit, parse_untracked,
};
use crate::run::classify;
use crate::testing::{Scratch, cases, latin, path_before};
use crate::{Git, GitOutcome};

const SHA: &str = "08ec32abb9b149de9d10310c24bd12bd1fe8d874";
const BLOB_OBJECT: &str = "08ec32abb9b149de9d10310c24bd12bd1fe8d874:./pkg/a.py";
const RANGE: &str = "08ec32abb9b149de9d10310c24bd12bd1fe8d874..HEAD";

type Row = (String, String, String, String, String);
type Call = fn(&Git, &Path) -> GitOutcome;
type Call2 = fn(&Git, &Path, &str) -> GitOutcome;

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Parsed {
    Changes(Vec<Row>),
    Malformed(String),
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Committed {
    Commit(String),
    Malformed(String),
}

fn side(side: Side) -> String {
    let name = match side {
        Side::Absent => "absent",
        Side::File => "file",
        Side::Link => "link",
        Side::Gitlink => "gitlink",
    };
    name.to_owned()
}

fn row(change: &Change) -> Row {
    let status = change.status.to_string();
    let (path, old_path) = (change.path.clone(), change.old_path.clone());
    (status, path, old_path, side(change.old), side(change.new))
}

fn calls() -> [(Call, Vec<&'static str>, usize); 6] {
    [
        (
            |git, root| git.verify(root, "--output=x"),
            vec![
                "rev-parse",
                "--verify",
                "--end-of-options",
                "--output=x^{commit}",
            ],
            4_096,
        ),
        (
            |git, root| git.changes(root, SHA),
            vec![
                "diff",
                "--raw",
                "-z",
                "-M",
                "--relative",
                "--end-of-options",
                SHA,
                "--",
            ],
            8_000_000,
        ),
        (
            |git, root| git.untracked(root),
            vec!["ls-files", "--others", "--exclude-standard", "-z"],
            8_000_000,
        ),
        (
            |git, root| git.blob(root, SHA, "pkg/a.py"),
            vec!["cat-file", "blob", BLOB_OBJECT],
            1_000_000,
        ),
        (
            |git, root| git.merge_base(root, SHA),
            vec!["merge-base", "--end-of-options", SHA, "HEAD"],
            4_096,
        ),
        (
            |git, root| git.commits_since(root, SHA),
            vec!["rev-list", "--count", "--end-of-options", RANGE],
            4_096,
        ),
    ]
}

#[test]
fn parse_changes_matches_the_reference_cases() {
    let cases: Vec<(String, Parsed)> = cases("changes", "parse_changes");
    assert!(!cases.is_empty());
    for (raw, expected) in cases {
        let raw = latin(&raw);
        let parsed = match parse_changes(&raw) {
            Ok(changes) => Parsed::Changes(changes.iter().map(row).collect()),
            Err(malformed) => Parsed::Malformed(malformed.to_string()),
        };
        assert_eq!(parsed, expected, "input {raw:?}");
    }
}

#[test]
fn parse_untracked_matches_the_reference_cases() {
    let cases: Vec<(String, Vec<Row>)> = cases("changes", "parse_untracked");
    assert!(!cases.is_empty());
    for (raw, expected) in cases {
        let raw = latin(&raw);
        let rows: Vec<Row> = parse_untracked(&raw).iter().map(row).collect();
        assert_eq!(rows, expected, "input {raw:?}");
    }
}

#[test]
fn parse_commit_matches_the_reference_cases() {
    let cases: Vec<(String, Committed)> = cases("changes", "parse_commit");
    assert!(!cases.is_empty());
    for (raw, expected) in cases {
        let raw = latin(&raw);
        let committed = match parse_commit(&raw) {
            Ok(name) => Committed::Commit(name),
            Err(malformed) => Committed::Malformed(malformed.to_string()),
        };
        assert_eq!(committed, expected, "input {raw:?}");
    }
}

#[test]
fn looks_binary_matches_the_reference_cases() {
    let cases: Vec<(String, bool)> = cases("changes", "looks_binary");
    assert!(!cases.is_empty());
    for (raw, expected) in cases {
        let raw = latin(&raw);
        assert_eq!(looks_binary(&raw), expected, "input of {} bytes", raw.len());
    }
}

#[test]
fn each_change_call_sends_its_arguments_after_the_directory() {
    let scratch = Scratch::new();
    let bin = scratch.fake_git("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_LOG\"\n", 0o755);
    let log = scratch.root.join("argv.log");
    let env = [("PATH", bin.as_os_str()), ("ARGV_LOG", log.as_os_str())];
    let git = Git::new(scratch.env_with(&env));
    let root = scratch.root.as_path();
    for (call, arguments, _) in calls() {
        assert_eq!(call(&git, root), GitOutcome::Printed(Vec::new()));
        let argv = fs::read_to_string(&log).expect("the fake git wrote its argv");
        let directory = root.to_str().expect("a UTF-8 scratch path");
        let expected: Vec<&str> = ["-C", directory].into_iter().chain(arguments).collect();
        let lines: Vec<&str> = argv.lines().collect();
        assert!(lines.ends_with(&expected), "{lines:?}");
    }
}

// The cap test above ties each call to its bounds. This pins the note each deadline gives.
#[test]
fn each_change_call_names_its_deadline_in_the_timeout_note() {
    let calls = [
        ("rev-parse", COMMIT, "git rev-parse timed out after 5.0s"),
        ("diff", LISTING, "git diff timed out after 30.0s"),
        ("ls-files", LISTING, "git ls-files timed out after 30.0s"),
        ("cat-file", BLOB, "git cat-file timed out after 5.0s"),
        ("merge-base", COMMIT, "git merge-base timed out after 5.0s"),
        ("rev-list", COMMIT, "git rev-list timed out after 5.0s"),
    ];
    for (subcommand, limits, expected) in calls {
        let outcome = classify(subcommand, limits, io::Outcome::TimedOut);
        let GitOutcome::Unfinished(unfinished) = outcome else {
            panic!("{subcommand}: a deadline is unfinished, not {outcome:?}");
        };
        assert_eq!(unfinished.to_string(), expected);
    }
}

#[test]
fn each_change_call_caps_stdout_at_its_own_bound() {
    let scratch = Scratch::new();
    let bin = scratch.fake_git("#!/bin/sh\nexec head -c \"$FAKE_BYTES\" /dev/zero\n", 0o755);
    let path = path_before(bin);
    let git_printing = |bytes: usize| {
        let count = bytes.to_string();
        let env = [
            ("PATH", path.as_os_str()),
            ("FAKE_BYTES", OsStr::new(&count)),
        ];
        Git::new(scratch.env_with(&env))
    };
    let root = scratch.root.as_path();
    for (call, arguments, cap) in calls() {
        let whole = call(&git_printing(cap), root);
        assert!(
            matches!(whole, GitOutcome::Printed(ref out) if out.len() == cap),
            "{arguments:?}"
        );
        let cut = call(&git_printing(cap + 1), root);
        assert!(
            matches!(cut, GitOutcome::Capped(ref out) if out.len() == cap),
            "{arguments:?}"
        );
    }
}

#[test]
fn a_real_listing_parses_into_changes() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a.py", "x = 1\n"), ("old.py", "y = 2\n")]);
    let git = scratch.git();
    let GitOutcome::Printed(head) = git.verify(&root, "HEAD") else {
        panic!("HEAD resolves in a repository with one commit");
    };
    let commit = parse_commit(&head).expect("one full object name");
    scratch.write("repo/a.py", "x = 2\n");
    scratch.write("repo/new.py", "z = 3\n");
    scratch.run_git(&root, &["mv", "old.py", "moved.py"]);

    let GitOutcome::Printed(listed) = git.changes(&root, &commit) else {
        panic!("the diff lists the changes");
    };
    let GitOutcome::Printed(untracked) = git.untracked(&root) else {
        panic!("ls-files lists the untracked files");
    };
    let changes: Vec<Row> = parse_changes(&listed)
        .expect("raw records")
        .iter()
        .map(row)
        .collect();
    let added: Vec<Row> = parse_untracked(&untracked).iter().map(row).collect();
    let file = || "file".to_owned();
    assert_eq!(
        changes,
        [
            (
                "M".to_owned(),
                "a.py".to_owned(),
                "a.py".to_owned(),
                file(),
                file()
            ),
            (
                "R".to_owned(),
                "moved.py".to_owned(),
                "old.py".to_owned(),
                file(),
                file()
            ),
        ]
    );
    let new = (
        "?".to_owned(),
        "new.py".to_owned(),
        "new.py".to_owned(),
        side(Side::Absent),
        file(),
    );
    assert_eq!(added, [new]);
    let blob = git.blob(&root, &commit, "a.py");
    assert_eq!(blob, GitOutcome::Printed(b"x = 1\n".to_vec()));
}

#[test]
fn merge_base_is_the_fork_point_and_commits_since_counts_head_past_it() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a.py", "x = 1\n")]);
    let git = scratch.git();
    let commit_of = |outcome: GitOutcome| match outcome {
        GitOutcome::Printed(stdout) => parse_commit(&stdout).expect("one full object name"),
        other => panic!("git printed no commit: {other:?}"),
    };
    let fork = commit_of(git.verify(&root, "HEAD"));
    scratch.run_git(&root, &["branch", "side"]);
    for text in ["x = 2\n", "x = 3\n"] {
        scratch.write("repo/a.py", text);
        scratch.run_git(&root, &["commit", "--quiet", "-am", "edit"]);
    }
    scratch.run_git(&root, &["checkout", "--quiet", "side"]);
    scratch.write("repo/b.py", "y = 1\n");
    scratch.run_git(&root, &["add", "b.py"]);
    scratch.run_git(&root, &["commit", "--quiet", "-m", "side"]);
    scratch.run_git(&root, &["checkout", "--quiet", "-"]);

    let side = commit_of(git.verify(&root, "side"));
    assert_eq!(commit_of(git.merge_base(&root, &side)), fork);
    let count = git.commits_since(&root, &fork);
    assert_eq!(count, GitOutcome::Printed(b"2\n".to_vec()));
}

// Each revision is bound after --end-of-options, so one that starts with "-" names no commit and
// never reaches git as an option such as --output.
#[test]
fn a_revision_that_starts_with_a_dash_is_an_unknown_revision_and_writes_no_file() {
    let scratch = Scratch::new();
    let root = scratch.repo("repo", &[("a.py", "x = 1\n")]);
    let git = scratch.git();
    let written = scratch.root.join("written");
    let option = format!("--output={}", written.display());
    let calls: [(&str, Call2); 2] = [
        ("diff", |git, root, revision| git.changes(root, revision)),
        ("rev-list", |git, root, revision| {
            git.commits_since(root, revision)
        }),
    ];
    for (subcommand, call) in calls {
        let outcome = call(&git, &root, &option);
        let GitOutcome::Failed(failure) = outcome else {
            panic!("{subcommand}: an unknown revision fails, not {outcome:?}");
        };
        let note = failure.to_string();
        assert!(note.contains(&format!("git {subcommand}")), "{note}");
        let stray = fs::read_dir(&scratch.root).expect("the scratch folder");
        let names: Vec<String> = stray
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|name| name.starts_with("written"))
            .collect();
        assert!(names.is_empty(), "{subcommand} wrote {names:?}");
    }
}
