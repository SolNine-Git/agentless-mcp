use git::{Change, Failure, GitOutcome, Side, Unfinished};
use json::{JsonForm, to_text};
use serde::Deserialize;

use super::{PairSpec, blob_from, commit_from, listing_from, pair_specs};
use crate::Error;
use crate::testing::{cases, latin};

const SHA: &str = "08ec32abb9b149de9d10310c24bd12bd1fe8d874";

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Answer {
    Text(String),
    Bytes(String),
    Row(String),
    Skipped(String),
    Error((String, String)),
}

// The outcome that the generator builds in its Python form for `case`.
fn outcome(case: &str, subcommand: &'static str, after: &'static str) -> GitOutcome {
    match case {
        "printed_sha" => GitOutcome::Printed(format!("{SHA}\n").into_bytes()),
        "printed_listing" => GitOutcome::Printed(b"a.py\0".to_vec()),
        "printed_junk" => GitOutcome::Printed(b"not one object name\n".to_vec()),
        "capped" => GitOutcome::Capped(vec![b'x'; 16]),
        "failed" => GitOutcome::Failed(Failure {
            subcommand,
            code: 128,
            detail: "fatal: bad revision 'x'".to_owned(),
        }),
        "not_installed" => GitOutcome::Unfinished(Unfinished::NotInstalled),
        "timed_out" => GitOutcome::Unfinished(Unfinished::TimedOut { subcommand, after }),
        "not_run" => GitOutcome::Unfinished(Unfinished::NotRun {
            subcommand,
            reason: "Permission denied".to_owned(),
        }),
        other => panic!("no outcome case {other}"),
    }
}

fn row(finding: &checks::PairFinding) -> Answer {
    Answer::Row(to_text(finding, JsonForm::Compact).expect("a row serializes"))
}

fn bytes(data: Vec<u8>) -> Answer {
    Answer::Bytes(data.into_iter().map(char::from).collect())
}

fn refused(error: Error) -> Answer {
    let Error::Kind(kind) = error else {
        panic!("a domain error, not {error:?}");
    };
    let name = match &kind {
        kernel::Error::OperationFailed(_) => "OperationFailed",
        kernel::Error::RevisionUnresolved(_) => "RevisionUnresolved",
        other => panic!("an unexpected kind {other:?}"),
    };
    Answer::Error((name.to_owned(), kind.to_string()))
}

fn committed(case: &str) -> Answer {
    match commit_from("main~2", outcome(case, "rev-parse", "5.0")) {
        Ok(Ok(commit)) => Answer::Text(commit),
        Ok(Err(gap)) => row(&gap),
        Err(error) => refused(error),
    }
}

fn listed(case: &str, command: &'static str) -> Answer {
    match listing_from(command, outcome(case, command, "30.0")) {
        Ok(data) => bytes(data),
        Err(gap) => row(&gap),
    }
}

fn blob(case: &str) -> Answer {
    match blob_from(outcome(case, "cat-file", "5.0")) {
        Ok(data) => bytes(data),
        Err(skipped) => Answer::Skipped(skipped),
    }
}

#[test]
fn each_git_outcome_gives_the_python_answer() {
    let rows: Vec<(String, String, Answer)> = cases("outcomes", "steps");
    for (case, step, expected) in rows {
        let found = match step.as_str() {
            "commit" => committed(&case),
            "diff" => listed(&case, "diff"),
            "ls-files" => listed(&case, "ls-files"),
            "blob" => blob(&case),
            other => panic!("no review step {other}"),
        };
        assert_eq!(found, expected, "{case} {step}");
    }
}

fn side(name: &str) -> Side {
    match name {
        "ABSENT" => Side::Absent,
        "FILE" => Side::File,
        "LINK" => Side::Link,
        "GITLINK" => Side::Gitlink,
        other => panic!("no side {other}"),
    }
}

type ChangeRow = (String, String, String, String, String);
type SpecRow = (String, Option<String>, Option<String>);

fn change((status, path, old_path, old, new): &ChangeRow) -> Change {
    Change {
        status: status.chars().next().expect("a status letter"),
        path: path.clone(),
        old_path: old_path.clone(),
        old: side(old),
        new: side(new),
    }
}

#[test]
fn each_change_gives_the_python_rows_and_specs() {
    let rows: Vec<(Vec<ChangeRow>, Vec<String>, Vec<SpecRow>)> = cases("outcomes", "pair_specs");
    for (input, expected_rows, expected_specs) in rows {
        let changes: Vec<Change> = input.iter().map(change).collect();
        let (found_rows, found_specs) = pair_specs(&changes);
        let shown: Vec<String> = found_rows
            .iter()
            .map(|found| to_text(found, JsonForm::Compact).expect("a row serializes"))
            .collect();
        let specs: Vec<PairSpec> = expected_specs
            .into_iter()
            .map(|(path, old_path, new_path)| PairSpec {
                path,
                old_path,
                new_path,
            })
            .collect();
        assert_eq!(shown, expected_rows, "{input:?}");
        assert_eq!(found_specs, specs, "{input:?}");
    }
}

#[test]
fn the_fixture_bytes_are_one_character_per_byte() {
    assert_eq!(latin("a\u{ff}"), [b'a', 0xff]);
}
