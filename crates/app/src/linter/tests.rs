use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use checks::{CheckGap, Detail, PairFinding, Severity};

use crate::testing::Sandbox;
use crate::{After, LintCandidate, LintForm, LintRequest, PairCheck, PairSelection, Repo, Tags};

type Answer = fn(&io::Spec) -> io::Outcome;

// A stand-in for ruff: it records each run and answers with `answer`, so no test starts a process.
struct FakeRuff {
    runs: Mutex<Vec<io::Spec>>,
    answer: Answer,
}

impl crate::ChildRunner for FakeRuff {
    fn run(&self, spec: &io::Spec) -> io::Outcome {
        let mut runs = self.runs.lock().unwrap_or_else(PoisonError::into_inner);
        runs.push(spec.clone());
        (self.answer)(spec)
    }
}

impl FakeRuff {
    fn new(answer: Answer) -> Arc<FakeRuff> {
        Arc::new(FakeRuff {
            runs: Mutex::new(Vec::new()),
            answer,
        })
    }

    fn runs(&self) -> Vec<io::Spec> {
        self.runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn printed(stdout: String) -> io::Outcome {
    io::Outcome::Exited {
        status: ExitStatus::from_raw(0),
        stdout: stdout.into_bytes(),
        stderr: Vec::new(),
    }
}

// Each `import NAME` line of stdin is an unused import, in ruff's JSON.
fn unused_imports(spec: &io::Spec) -> io::Outcome {
    let text = String::from_utf8(spec.stdin.clone().expect("the text on stdin")).expect("UTF-8");
    let findings: Vec<String> = (text.lines().zip(1..))
        .filter_map(|(line, row)| {
            let name = line.strip_prefix("import ")?;
            Some(format!(
                "{{\"code\": \"F401\", \"message\": \"`{name}` imported but unused\", \
                 \"location\": {{\"row\": {row}, \"column\": 8}}, \"fix\": {{\"message\": \
                 \"Remove unused import: `{name}`\"}}}}"
            ))
        })
        .collect();
    printed(format!("[{}]", findings.join(", ")))
}

fn hangs(_: &io::Spec) -> io::Outcome {
    io::Outcome::TimedOut
}

fn fails(_: &io::Spec) -> io::Outcome {
    io::Outcome::Exited {
        status: ExitStatus::from_raw(2 << 8),
        stdout: Vec::new(),
        stderr: b"ruff failed\n  Cause: unknown rule selector\n".to_vec(),
    }
}

fn missing(_: &io::Spec) -> io::Outcome {
    let error = std::io::Error::from(std::io::ErrorKind::NotFound);
    io::Outcome::NotRun(io::NotRun::ProgramNotFound(error))
}

struct Linted {
    candidate: LintCandidate,
    root: PathBuf,
    home: PathBuf,
}

// One proposed text for `path` in a repository of `files`, linted by `ruff` alone.
fn lint(
    files: &[(&str, &str)],
    proposed: (&str, &str),
    ruff: &Arc<FakeRuff>,
    budget: Option<Duration>,
) -> Linted {
    lint_in(&Sandbox::new(), files, proposed, ruff, budget)
}

fn lint_in(
    sandbox: &Sandbox,
    files: &[(&str, &str)],
    (path, after): (&str, &str),
    ruff: &Arc<FakeRuff>,
    budget: Option<Duration>,
) -> Linted {
    let root = sandbox.repo("repo", files);
    let mut services = sandbox.services(&root);
    services.runner = Arc::clone(ruff) as Arc<dyn crate::ChildRunner>;
    let repo = Repo::load(&services, &root).expect("the repository loads");
    let tags = Tags::open(&services, &repo)
        .value
        .expect("the cache root is known");
    let request = LintRequest {
        form: LintForm::Proposed {
            path: path.to_owned(),
            after: After::Text(after.to_owned()),
        },
        checks: PairSelection::of(&[PairCheck::LinterDelta]).expect("a selection"),
        budget,
    };
    let report = crate::lint(&services, &repo, &tags, &request).value;
    let mut candidates = report.expect("the lint runs").candidates;
    Linted {
        candidate: candidates.remove(0),
        root,
        home: sandbox.path().join("home"),
    }
}

fn only_row(candidate: &LintCandidate) -> &PairFinding {
    let [row] = &candidate.findings[..] else {
        panic!("{:?}", candidate.findings);
    };
    row
}

fn gap_of(candidate: &LintCandidate) -> (CheckGap, String) {
    let row = only_row(candidate);
    assert_eq!(row.finding.severity, Severity::NotChecked, "{row:?}");
    let Some(Detail::Gap(gap)) = row.detail else {
        panic!("{row:?} has no gap");
    };
    (gap, row.finding.message.clone())
}

const RUFF_TOML: (&str, &str) = ("ruff.toml", "[lint]\nselect = [\"F\"]\n");
const OLD: (&str, &str) = ("src/app.py", "x = 1\n");

#[test]
fn an_added_unused_import_gives_one_advisory_that_names_its_code_and_fix() {
    let ruff = FakeRuff::new(unused_imports);
    let linted = lint(
        &[RUFF_TOML, OLD],
        ("src/app.py", "import os\nx = 1\n"),
        &ruff,
        None,
    );
    let row = only_row(&linted.candidate);
    assert_eq!(row.finding.severity, Severity::Advisory);
    assert_eq!(
        (row.finding.line, row.finding.message.as_str()),
        (
            1,
            "ruff F401: `os` imported but unused. Fix: Remove unused import: `os`"
        )
    );
    assert_eq!(ruff.runs().len(), 2, "one run for each text");
}

#[test]
fn an_edit_that_keeps_old_findings_and_adds_none_is_clean() {
    let ruff = FakeRuff::new(unused_imports);
    let old = ("src/app.py", "import os\nx = 1\n");
    let linted = lint(
        &[RUFF_TOML, old],
        ("src/app.py", "import os\nx = 2\n"),
        &ruff,
        None,
    );
    assert_eq!(linted.candidate.findings, []);
    let observed = linted
        .candidate
        .observed
        .expect("the check linted the file");
    assert_eq!(observed.linter_keys.map(|keys| keys.len()), Some(1));
}

#[test]
fn each_run_lints_one_text_on_stdin_from_the_root_with_only_home_and_path() {
    let ruff = FakeRuff::new(unused_imports);
    let linted = lint(&[RUFF_TOML, OLD], ("src/app.py", "y = 2\n"), &ruff, None);
    let runs = ruff.runs();
    let texts: Vec<Option<Vec<u8>>> = runs.iter().map(|spec| spec.stdin.clone()).collect();
    assert_eq!(
        texts,
        [Some(b"x = 1\n".to_vec()), Some(b"y = 2\n".to_vec())]
    );
    let spec = &runs[0];
    assert_eq!(spec.program, OsString::from("ruff"));
    let args: Vec<&str> = (spec.args.iter())
        .map(|arg| arg.to_str().expect("UTF-8"))
        .collect();
    let expected = [
        "check",
        "--force-exclude",
        "--stdin-filename",
        "src/app.py",
        "--output-format",
        "json",
        "--exit-zero",
        "-",
    ];
    assert_eq!(args, expected);
    assert_eq!(spec.cwd, linted.root);
    let names: Vec<&OsString> = spec.env.iter().map(|(name, _)| name).collect();
    assert_eq!(names, ["HOME", "PATH"]);
    assert_eq!(spec.env[0].1, linted.home.into_os_string());
    assert_eq!(spec.timeout, Duration::from_secs(2));
}

#[test]
fn the_repository_program_runs_before_the_one_on_path() {
    let ruff = FakeRuff::new(unused_imports);
    let local = (".venv/bin/ruff", "#!/bin/sh\n");
    let linted = lint(
        &[RUFF_TOML, OLD, local],
        ("src/app.py", "y = 2\n"),
        &ruff,
        None,
    );
    let program = linted.root.join(".venv/bin/ruff").into_os_string();
    assert!(ruff.runs().iter().all(|spec| spec.program == program));
}

#[test]
fn a_config_counts_in_the_folder_of_the_file_or_above_it_up_to_the_root() {
    let configs = [
        ("src/.ruff.toml", "line-length = 100\n"),
        ("ruff.toml", "line-length = 100\n"),
        ("pyproject.toml", "[tool.ruff.lint]\nselect = [\"F\"]\n"),
        ("src/pyproject.toml", "[tool.ruff]\n"),
        ("pyproject.toml", "[project]\nname = \"a\"\n[tool.ruff\n"),
    ];
    for config in configs {
        let ruff = FakeRuff::new(unused_imports);
        let linted = lint(&[config, OLD], ("src/app.py", "y = 2\n"), &ruff, None);
        assert_eq!(linted.candidate.findings, [], "{config:?}");
        assert_eq!(ruff.runs().len(), 2, "{config:?}");
    }
}

#[test]
fn no_config_up_to_the_root_is_a_no_linter_config_gap_and_starts_no_run() {
    // The sandbox folder above the repository holds a config, which the search never reaches.
    let unrelated = [
        ("pyproject.toml", "[project]\nname = \"a\"\n[tool.black]\n"),
        ("other/ruff.toml", "line-length = 100\n"),
    ];
    let ruff = FakeRuff::new(unused_imports);
    let sandbox = Sandbox::new();
    sandbox.put("ruff.toml", "line-length = 100\n");
    let linted = lint_in(
        &sandbox,
        &unrelated,
        ("src/app.py", "import os\n"),
        &ruff,
        None,
    );
    let (gap, message) = gap_of(&linted.candidate);
    assert_eq!(gap, CheckGap::NoLinterConfig);
    assert!(message.contains("found no ruff config"), "{message}");
    assert_eq!(ruff.runs().len(), 0);
}

#[test]
fn no_program_is_a_no_linter_binary_gap() {
    let ruff = FakeRuff::new(missing);
    let linted = lint(
        &[RUFF_TOML, OLD],
        ("src/app.py", "import os\n"),
        &ruff,
        None,
    );
    assert_eq!(gap_of(&linted.candidate).0, CheckGap::NoLinterBinary);
}

#[test]
fn a_run_that_hangs_or_fails_is_a_linter_failed_gap_with_its_reason() {
    let cases: [(Answer, &str); 2] = [
        (hangs, "it ran past its timeout of 2.0 s."),
        (fails, "it ended with exit status: 2; stderr: ruff failed."),
    ];
    for (answer, reason) in cases {
        let ruff = FakeRuff::new(answer);
        let linted = lint(
            &[RUFF_TOML, OLD],
            ("src/app.py", "import os\n"),
            &ruff,
            None,
        );
        let (gap, message) = gap_of(&linted.candidate);
        assert_eq!(gap, CheckGap::LinterFailed);
        assert!(message.ends_with(reason), "{message}");
        assert_eq!(
            ruff.runs().len(),
            1,
            "the old text failed, so the new text did not run"
        );
    }
}

#[test]
fn each_run_gets_at_most_half_the_budget_left_and_a_spent_budget_starts_none() {
    let ruff = FakeRuff::new(unused_imports);
    lint(
        &[RUFF_TOML, OLD],
        ("src/app.py", "y = 2\n"),
        &ruff,
        Some(Duration::from_secs(3)),
    );
    let timeout = ruff.runs()[0].timeout;
    let half = Duration::from_millis(1500);
    assert!(timeout <= half && timeout > half / 2, "{timeout:?}");
    let ruff = FakeRuff::new(unused_imports);
    let budget = Some(Duration::ZERO);
    let linted = lint(&[RUFF_TOML, OLD], ("src/app.py", "y = 2\n"), &ruff, budget);
    let (gap, message) = gap_of(&linted.candidate);
    assert_eq!(gap, CheckGap::LinterFailed);
    assert!(
        message.ends_with("the call had no time left to run it."),
        "{message}"
    );
    assert_eq!(ruff.runs().len(), 0);
}

#[test]
fn a_file_in_a_language_with_no_linter_row_is_not_kept() {
    let ruff = FakeRuff::new(unused_imports);
    let linted = lint(
        &[RUFF_TOML, ("src/app.rs", "fn a() {}\n")],
        ("src/app.rs", "fn b() {}\n"),
        &ruff,
        None,
    );
    assert_eq!(linted.candidate.findings, []);
    assert_eq!(ruff.runs().len(), 0);
}
