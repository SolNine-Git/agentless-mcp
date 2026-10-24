use std::collections::BTreeMap;
use std::fs;

use serde_json::Value;

use super::{CHURN_WINDOW_DAYS, Churn, commit_time, tally};
use crate::recipe::{FIXTURE, Repos};
use crate::run::classify;
use crate::snapshot::RECEIPT;
use crate::testing::{Scratch, cases, path_before};
use crate::{Git, GitOutcome};

type Counts = BTreeMap<String, (usize, Option<i64>)>;

fn counts(churn: Option<BTreeMap<String, Churn>>) -> Option<Counts> {
    churn.map(|counts| {
        counts
            .into_iter()
            .map(|(path, churn)| (path, (churn.commits, churn.last_commit_ts)))
            .collect()
    })
}

#[test]
fn each_churn_answer_over_real_repositories_matches_the_reference() {
    let repos = Repos::build();
    let git = repos.scratch.git();
    let all: Vec<(String, Vec<String>, Option<Counts>)> = cases(FIXTURE, "churn");
    assert!(!all.is_empty());
    for (repo, paths, expected) in all {
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let got = counts(git.churn(&repos.path(&repo), &paths));
        assert_eq!(got, expected, "{repo} {paths:?}");
    }
}

#[test]
fn the_churn_call_sends_the_reference_arguments_after_the_directory() {
    let scratch = Scratch::new();
    let bin = scratch.fake_git("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_LOG\"\n", 0o755);
    let log = scratch.root.join("argv.log");
    let env = [("PATH", bin.as_os_str()), ("ARGV_LOG", log.as_os_str())];
    let git = Git::new(scratch.env_with(&env));
    let all: Vec<(String, Value, Vec<String>)> = cases(FIXTURE, "argv");
    let (_, input, expected) = all
        .iter()
        .find(|(call, _, _)| call == "churn")
        .expect("a churn argv case");
    let paths: Vec<&str> = input
        .as_array()
        .expect("a list of paths")
        .iter()
        .map(|path| path.as_str().expect("a path"))
        .collect();
    let answered = git
        .churn(&scratch.root, &paths)
        .expect("an empty log answers");
    assert!(answered.values().all(|churn| churn.commits == 0));
    let argv = fs::read_to_string(&log).expect("the fake git wrote its argv");
    let lines: Vec<&str> = argv.lines().collect();
    let directory = scratch.root.to_str().expect("a UTF-8 scratch path");
    let wanted: Vec<&str> = ["-C", directory]
        .into_iter()
        .chain(expected.iter().map(String::as_str))
        .collect();
    assert!(lines.ends_with(&wanted), "{lines:?}");
    assert!(lines.contains(&format!("--since={CHURN_WINDOW_DAYS}.days").as_str()));
}

#[test]
fn the_churn_deadline_is_the_receipt_deadline() {
    let outcome = classify("log", RECEIPT, io::Outcome::TimedOut);
    let GitOutcome::Unfinished(unfinished) = outcome else {
        panic!("a deadline is unfinished, not {outcome:?}");
    };
    assert_eq!(unfinished.to_string(), "git log timed out after 5.0s");
}

// One commit line of 16 bytes, then spaces up to FAKE_BYTES: strip removes them from the path.
#[test]
fn a_churn_log_over_its_cap_is_unknown_not_partial() {
    let scratch = Scratch::new();
    let body = "#!/bin/sh\nprintf '\\000%s\\na.py' 1700000000\n\
                head -c $((FAKE_BYTES - 16)) /dev/zero | tr '\\0' ' '\n";
    let path = path_before(scratch.fake_git(body, 0o755));
    let git_printing = |bytes: usize| {
        let count = bytes.to_string();
        let env = [("PATH", path.as_os_str()), ("FAKE_BYTES", count.as_ref())];
        Git::new(scratch.env_with(&env))
    };
    let at_cap = counts(git_printing(RECEIPT.cap).churn(&scratch.root, &["a.py"]));
    let expected: Counts = [("a.py".to_owned(), (1, Some(1_700_000_000)))].into();
    assert_eq!(at_cap, Some(expected));
    let over = git_printing(RECEIPT.cap + 1).churn(&scratch.root, &["a.py"]);
    assert_eq!(over, None);
}

#[test]
fn a_stamp_is_ascii_digits_and_fits_i64() {
    assert_eq!(commit_time("1700000000"), Some(1_700_000_000));
    assert_eq!(commit_time("0123"), Some(123));
    assert_eq!(commit_time(""), None);
    assert_eq!(commit_time("12a"), None);
    assert_eq!(commit_time("\u{661}\u{662}"), None);
    assert_eq!(commit_time("99999999999999999999"), None);
}

#[test]
fn a_line_counts_only_after_a_stamp_and_an_empty_line_never_counts() {
    let log = "a.py\n\u{0}bad\na.py\n\n\u{0} 7\n\u{0} \n\u{0}5\n a.py \n\u{0}4\na.py\n";
    let got = tally(log, &["a.py", ""]);
    let want = |commits, last_commit_ts| Churn {
        commits,
        last_commit_ts,
    };
    let expected: BTreeMap<String, Churn> = [
        ("a.py".to_owned(), want(2, Some(5))),
        (String::new(), want(0, None)),
    ]
    .into();
    assert_eq!(got, expected);
}
