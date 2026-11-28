use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

use prompts::Linter;
use serde::Deserialize;

use super::{LintedPair, LintedSides, LinterFailure, LinterFinding, NotLinted, linter_delta, ran};
use crate::{CheckGap, Detail, LinterKey, LinterState, Pair, Severity, SideText, TextPair};

const CAP: usize = 1 << 20;
const TIMEOUT: Duration = Duration::from_secs(2);

// One recorded run of a local ruff on an input of the fixture: its stdout and each finding as
// code, message, row and fix message.
#[derive(Deserialize)]
struct Recorded {
    version: String,
    input: String,
    stdout: String,
    findings: Vec<(String, String, u32, Option<String>)>,
}

fn ruff() -> &'static Linter {
    let row = prompts::linter_rules()
        .iter()
        .find(|rules| rules.language == "python")
        .expect("python has a linter row");
    row.linter.as_ref().expect("python's row is a linter")
}

fn exited(code: i32, stdout: &str, stderr: &str) -> io::Outcome {
    io::Outcome::Exited {
        status: ExitStatus::from_raw(code << 8),
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

fn found(code: &str, row: u32, fix: Option<&str>) -> LinterFinding {
    LinterFinding {
        code: code.to_owned(),
        message: format!("{code} message"),
        row,
        fix: fix.map(str::to_owned),
    }
}

fn pair(old: Option<&str>, new: Option<&str>) -> Vec<Pair> {
    let text = TextPair {
        path: "src/a.py".to_owned(),
        old: old.map(str::to_owned),
        new: new.map(str::to_owned),
    };
    Pair::measure_all(vec![text], |sides| {
        sides.iter().map(SideText::measure).collect()
    })
}

fn delta(old: Option<&str>, new: &str, linted: Result<LintedSides, NotLinted>) -> crate::Outcome {
    let pairs = pair(old, Some(new));
    let linted = [LintedPair {
        pair: &pairs[0],
        linter: ruff(),
        linted,
    }];
    linter_delta(&linted).expect("the run succeeds")
}

fn sides(old: Vec<LinterFinding>, new: Vec<LinterFinding>) -> LintedSides {
    LintedSides { old, new }
}

fn shown(outcome: &crate::Outcome) -> Vec<(u32, String, String)> {
    (outcome.findings.iter())
        .map(|row| {
            let Some(Detail::Linter(key)) = &row.detail else {
                panic!("{row:?} has no linter key");
            };
            assert_eq!(row.finding.severity, Severity::Advisory, "{row:?}");
            (
                row.finding.line,
                key.key.clone(),
                row.finding.message.clone(),
            )
        })
        .collect()
}

#[test]
fn the_json_of_each_recorded_ruff_version_parses_into_its_findings() {
    let recorded: Vec<Recorded> =
        text::testing::cases(env!("CARGO_MANIFEST_DIR"), "ruff", "outputs");
    let versions: Vec<&str> = recorded.iter().map(|run| run.version.as_str()).collect();
    for version in ["0.15.16", "0.16.3", "0.17.6"] {
        assert!(versions.contains(&version), "{version}: {versions:?}");
    }
    for run in &recorded {
        let parsed = ran(exited(0, &run.stdout, ""), TIMEOUT, CAP)
            .unwrap_or_else(|error| panic!("{} {}: {error:?}", run.version, run.input));
        let expected: Vec<LinterFinding> = (run.findings.iter())
            .map(|(code, message, row, fix)| LinterFinding {
                code: code.clone(),
                message: message.clone(),
                row: *row,
                fix: fix.clone(),
            })
            .collect();
        assert_eq!(parsed, expected, "{} {}", run.version, run.input);
    }
}

#[test]
fn a_run_that_does_not_exit_0_with_parsed_json_is_a_failure_with_its_reason() {
    let failed = |outcome| ran(outcome, TIMEOUT, CAP).unwrap_err();
    assert_eq!(
        failed(exited(2, "", "ruff failed\n  Cause: bad selector\n")),
        NotLinted::Failed(LinterFailure::Exited {
            status: "exit status: 2".to_owned(),
            stderr: Some("ruff failed".to_owned()),
        })
    );
    assert!(matches!(
        failed(exited(0, "[{\"code\": 1}]", "")),
        NotLinted::Failed(LinterFailure::Unparsed(_))
    ));
    assert_eq!(
        failed(io::Outcome::TimedOut),
        NotLinted::Failed(LinterFailure::TimedOut(TIMEOUT))
    );
    let truncated = io::Outcome::Truncated { stdout: Vec::new() };
    assert_eq!(
        failed(truncated),
        NotLinted::Failed(LinterFailure::Capped(CAP))
    );
    let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
    let not_found = io::Outcome::NotRun(io::NotRun::ProgramNotFound(missing));
    assert_eq!(failed(not_found), NotLinted::NoBinary);
}

#[test]
fn an_added_unused_import_gives_one_advisory_with_its_code_and_fix() {
    let old = "import sys\n\nprint(sys.argv)\n";
    let new = "import os\nimport sys\n\nprint(sys.argv)\n";
    let fix = "Remove unused import: `os`";
    let outcome = delta(
        Some(old),
        new,
        Ok(sides(vec![], vec![found("F401", 1, Some(fix))])),
    );
    let expected = (
        1,
        "import os".to_owned(),
        "ruff F401: F401 message. Fix: Remove unused import: `os`".to_owned(),
    );
    assert_eq!(shown(&outcome), [expected]);
    assert_eq!(outcome.findings[0].finding.evidence, "code F401");
}

#[test]
fn an_edit_that_keeps_the_old_findings_and_adds_none_is_clean() {
    let old = "import os\nx = 1\n";
    let new = "import os\nx = 2\ny = 3\n";
    let unused = |row| found("F401", row, None);
    let outcome = delta(Some(old), new, Ok(sides(vec![unused(1)], vec![unused(1)])));
    assert_eq!(outcome.findings, []);
}

#[test]
fn a_finding_on_a_moved_line_is_no_finding_and_a_copied_line_is_one() {
    let old = "import os\n";
    let new = "\n\n    import   os\nimport os\n";
    let unused = |row| found("F401", row, None);
    let moved = delta(Some(old), new, Ok(sides(vec![unused(1)], vec![unused(3)])));
    assert_eq!(moved.findings, []);
    let copied = delta(
        Some(old),
        new,
        Ok(sides(vec![unused(1)], vec![unused(3), unused(4)])),
    );
    let lines: Vec<u32> = shown(&copied).iter().map(|(line, ..)| *line).collect();
    assert_eq!(lines, [4]);
}

#[test]
fn every_syntax_error_of_one_text_is_one_key() {
    let errors = |rows: &[u32]| -> Vec<LinterFinding> {
        (rows.iter())
            .map(|row| found("invalid-syntax", *row, None))
            .collect()
    };
    let new = "def f(:\nx = (\n";
    let added = delta(Some("x = 1\n"), new, Ok(sides(vec![], errors(&[1, 1, 2]))));
    let expected = (
        1,
        String::new(),
        "ruff invalid-syntax: invalid-syntax message".to_owned(),
    );
    assert_eq!(shown(&added), [expected]);
    let kept = delta(
        Some("def f(:\n"),
        new,
        Ok(sides(errors(&[1]), errors(&[1, 1, 2]))),
    );
    assert_eq!(kept.findings, []);
}

#[test]
fn a_row_past_the_last_line_keys_on_an_empty_line() {
    let outcome = delta(
        None,
        "x = 1",
        Ok(sides(vec![], vec![found("W292", 9, None)])),
    );
    assert_eq!(shown(&outcome)[0].1, "");
}

#[test]
fn the_lines_break_where_the_linter_counts_rows() {
    let new = "a = 1\r\nb = 2\rimport os\n";
    let outcome = delta(None, new, Ok(sides(vec![], vec![found("F401", 3, None)])));
    assert_eq!(shown(&outcome)[0].1, "import os");
}

#[test]
fn the_observed_keys_hold_every_finding_of_the_new_text() {
    let old = "import os\n";
    let new = "import os\nimport  sys\n";
    let unused = |row| found("F401", row, None);
    let outcome = delta(
        Some(old),
        new,
        Ok(sides(vec![unused(1)], vec![unused(1), unused(2)])),
    );
    let state = |key: &str| LinterState {
        path: "src/a.py".to_owned(),
        code: "F401".to_owned(),
        key: key.to_owned(),
    };
    let expected = vec![state("import os"), state("import sys")];
    assert_eq!(outcome.observed.linter_keys, Some(expected));
    let key = LinterKey {
        code: "F401".to_owned(),
        key: "import sys".to_owned(),
    };
    assert_eq!(outcome.findings[0].detail, Some(Detail::Linter(key)));
}

#[test]
fn a_pair_that_was_not_linted_gives_one_not_checked_row_with_its_gap() {
    let failure = LinterFailure::Exited {
        status: "exit status: 2".to_owned(),
        stderr: Some("ruff failed".to_owned()),
    };
    let cases = [
        (
            NotLinted::NoConfig,
            CheckGap::NoLinterConfig,
            "not checked: linter_delta found no ruff config from the folder of src/a.py up to \
             the repository root (ruff.toml, .ruff.toml, or pyproject.toml with a [tool.ruff] \
             table), so it did not lint the file.",
        ),
        (
            NotLinted::NoBinary,
            CheckGap::NoLinterBinary,
            "not checked: linter_delta found no ruff program at .venv/bin/ruff or on PATH, so \
             it did not lint src/a.py.",
        ),
        (
            NotLinted::Failed(failure),
            CheckGap::LinterFailed,
            "not checked: linter_delta could not lint src/a.py with ruff: it ended with exit \
             status: 2; stderr: ruff failed.",
        ),
        (
            NotLinted::Failed(LinterFailure::BudgetSpent),
            CheckGap::LinterFailed,
            "not checked: linter_delta could not lint src/a.py with ruff: the call had no time \
             left to run it.",
        ),
    ];
    for (not_linted, gap, message) in cases {
        let outcome = delta(None, "x = 1\n", Err(not_linted));
        let [row] = &outcome.findings[..] else {
            panic!("{:?}", outcome.findings);
        };
        assert_eq!(row.finding.severity, Severity::NotChecked);
        assert_eq!(row.detail, Some(Detail::Gap(gap)));
        assert_eq!(row.finding.message, message);
        assert_eq!(outcome.observed.linter_keys, None);
    }
}

#[test]
fn a_deleted_file_gives_no_row() {
    let pairs = pair(Some("import os\n"), None);
    let linted = [LintedPair {
        pair: &pairs[0],
        linter: ruff(),
        linted: Err(NotLinted::NoConfig),
    }];
    assert_eq!(
        linter_delta(&linted).expect("the run succeeds").findings,
        []
    );
}

// A new language file fails here until its linter row or its gap is in the rule file.
#[test]
fn every_language_file_has_a_linter_row_or_a_gap() {
    let rows: std::collections::BTreeSet<String> = (prompts::linter_rules().iter())
        .map(|rules| rules.language.to_owned())
        .collect();
    assert_eq!(rows, crate::tests::language_files());
}

#[test]
fn two_pairs_of_one_path_stop_the_run() {
    let pairs = pair(None, Some("x = 1\n"));
    let linted = |pair| LintedPair {
        pair,
        linter: ruff(),
        linted: Err(NotLinted::NoConfig),
    };
    let both = [linted(&pairs[0]), linted(&pairs[0])];
    assert!(matches!(
        linter_delta(&both),
        Err(crate::Error::RepeatedPath { .. })
    ));
}
