use app::{ApplyResult, CheckReport, FileCheck, SyntaxVerdict};
use patch::{Regression, VoteCandidate};
use serde::Deserialize;

use super::rows::cases;
use crate::{check_text, vote_summary, vote_text};

// id, index, applied, measured, equivalence key, regression passed, reproduction passed.
type CandidateRow = (String, i64, bool, bool, Option<String>, bool, bool);

#[derive(Deserialize)]
struct VoteCase(Vec<CandidateRow>, bool, String, String);

fn candidate(row: &CandidateRow) -> VoteCandidate {
    let (id, index, applied, measured, key, passed, reproduced) = row.clone();
    let regression = match (measured, passed) {
        (false, _) => Regression::NotMeasured,
        (true, false) => Regression::Failed,
        (true, true) => Regression::Passed,
    };
    VoteCandidate {
        id,
        index,
        applied,
        equivalence_key: key,
        regression,
        reproduction_passed: reproduced,
    }
}

#[test]
fn each_vote_renders_as_the_fixture_says() {
    let all: Vec<VoteCase> = cases("write", "vote");
    assert!(all.len() > 5);
    for VoteCase(rows, repro_valid, text, summary) in all {
        let candidates: Vec<VoteCandidate> = rows.iter().map(candidate).collect();
        let report = patch::rank(&candidates, repro_valid);
        assert_eq!(vote_text(&report), text, "{rows:?}");
        assert_eq!(vote_summary(&report), summary, "{rows:?}");
    }
}

// language, old errors, new errors, ok, detail, checked, first error line.
type VerdictRow = (String, usize, usize, bool, String, bool, Option<u64>);
type FileRow = (String, Option<VerdictRow>, String);

fn file_check((path, verdict, error): &FileRow) -> FileCheck {
    let verdict = verdict.clone().map(|row| {
        let (language, old_errors, new_errors, ok, detail, checked, first_error_line) = row;
        SyntaxVerdict {
            language: language.leak(),
            old_errors,
            new_errors,
            ok,
            checked,
            first_error_line,
            detail,
        }
    });
    FileCheck {
        path: path.clone(),
        verdict,
        error: error.clone(),
    }
}

#[test]
fn each_check_renders_as_the_fixture_says() {
    let all: Vec<(Vec<FileRow>, String)> = cases("write", "check");
    assert!(all.len() > 3);
    for (rows, expected) in all {
        let report = CheckReport {
            files: rows.iter().map(file_check).collect(),
            result: ApplyResult::default(),
        };
        assert_eq!(check_text(&report), expected, "{rows:?}");
    }
}
