mod ports;

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use io::{RunResult, RunStatus, TestCommand, TestRun, TestRunError};
use serde::Deserialize;
use serde_json::{Value, json};

use super::candidates::load_candidates;
use super::plan::{fingerprint, test_config_paths, test_config_reason};
use super::{TestRunner, ValidateRequest, validate};
use crate::clock::PinnedClock;
use crate::testing::{MANIFEST_DIR, Sandbox, buildable_cases};
use crate::{Error, IntArg, Repo, Services};

const RECEIPT: &str = "<RECEIPT>";
const STATUSES: [RunStatus; 4] = [
    RunStatus::Passed,
    RunStatus::Failed,
    RunStatus::Timeout,
    RunStatus::Error,
];

// The kind is the variant name, which the generator writes as the class name without "Error".
fn refusal(error: Error) -> Value {
    let Error::Kind(kind) = error else {
        panic!("a defect: {error:?}");
    };
    let shown = format!("{kind:?}");
    let name = shown.split('(').next().unwrap_or_default();
    json!({"error": [name, kind.to_string()]})
}

// The sandbox path and each random worktree name, as the generator masks them.
fn masked(sandbox: &Sandbox, text: &str) -> String {
    let text = text.replace(&sandbox.path().display().to_string(), "<SANDBOX>");
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find("wt-") {
        out.push_str(&rest[..at + 3]);
        rest = &rest[at + 3..];
        let hex = rest
            .bytes()
            .take(32)
            .filter(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if hex.count() == 32 {
            out.push_str("<ID>");
            rest = &rest[32..];
        }
    }
    out.push_str(rest);
    out
}

fn shown(sandbox: &Sandbox, value: &Value) -> Value {
    let text = serde_json::to_string(value).expect("a value serializes");
    serde_json::from_str(&masked(sandbox, &text)).expect("masked JSON is JSON")
}

// The chmod steps run last; `chmods` with a mode undoes them before the sandbox is removed.
fn built(steps: &[Vec<Value>]) -> Sandbox {
    let sandbox = Sandbox::new();
    for step in steps.iter().filter(|step| step[0] != "chmod") {
        sandbox.apply(step);
    }
    chmods(&sandbox, steps, None);
    sandbox
}

fn chmods(sandbox: &Sandbox, steps: &[Vec<Value>], mode: Option<u32>) {
    for step in steps.iter().filter(|step| step[0] == "chmod") {
        let wanted = step[2].as_u64().and_then(|mode| u32::try_from(mode).ok());
        let mode = mode.or(wanted).expect("a mode");
        sandbox.chmod(step[1].as_str().expect("a place"), mode);
    }
}

fn binds_modes(steps: &[Vec<Value>]) -> bool {
    !steps.iter().any(|step| step[0] == "chmod") || Sandbox::new().modes_bind()
}

#[derive(Debug, Deserialize)]
struct Load {
    name: String,
    steps: Vec<Vec<Value>>,
    target: String,
    expected: Value,
}

#[test]
fn each_candidate_layout_loads_as_the_reference_loads_it() {
    let cases = buildable_cases("validate/inputs", "candidates", |case: &Load| &case.steps);
    for case in cases.iter().filter(|case| binds_modes(&case.steps)) {
        let sandbox = built(&case.steps);
        let services = sandbox.services(sandbox.path());

        let found = match load_candidates(&services, Path::new(&case.target)) {
            Ok(loaded) => {
                let rows = loaded
                    .iter()
                    .map(|entry| json!([entry.id, entry.index, entry.text]));
                json!({"candidates": rows.collect::<Vec<Value>>()})
            }
            Err(error) => refusal(error),
        };

        chmods(&sandbox, &case.steps, Some(0o644));
        assert_eq!(shown(&sandbox, &found), case.expected, "{}", case.name);
    }
}

#[test]
fn a_candidates_directory_or_entry_whose_stat_fails_is_unreadable_not_absent() {
    let sandbox = Sandbox::new();
    let services = sandbox.services(sandbox.path());
    sandbox.put("hidden/c/a.txt", "x");
    sandbox.put("hidden/b.txt", "x");
    sandbox.mkdir("cands");
    let link = ["symlink", "cands/b.txt", "<SANDBOX>/hidden/b.txt"];
    sandbox.apply(&link.map(Value::from));
    if !sandbox.modes_bind() {
        return;
    }
    sandbox.chmod("hidden", 0o000);
    let directory = load_candidates(&services, &sandbox.path().join("hidden/c"));
    let entry = load_candidates(&services, &sandbox.path().join("cands"));
    sandbox.chmod("hidden", 0o755);
    let refused = |found: Result<_, Error>| shown(&sandbox, &refusal(found.expect_err("refused")));
    let expected = |path: &str| {
        let text = format!("cannot read <SANDBOX>/{path}: Permission denied");
        json!({"error": ["InputUnreadable", text]})
    };
    assert_eq!(refused(directory), expected("hidden/c"));
    assert_eq!(refused(entry), expected("cands/b.txt"));
}

#[test]
fn each_execution_key_hashes_to_the_reference_fingerprint() {
    let cases: Vec<(Vec<(String, String)>, String)> =
        io::testing::cases(MANIFEST_DIR, "validate/inputs", "fingerprints");
    assert_eq!(cases.len(), 4);
    for (key, expected) in &cases {
        assert_eq!(&fingerprint(key), expected, "{key:?}");
    }
}

#[test]
fn each_path_set_names_the_reference_test_config_paths() {
    let cases: Vec<(Vec<String>, Vec<String>, Option<String>)> =
        io::testing::cases(MANIFEST_DIR, "validate/inputs", "test_config");
    for (paths, expected, reason) in &cases {
        let named = test_config_paths(paths.iter().map(String::as_str));

        assert_eq!(&named, expected, "{paths:?}");
        let found = (!named.is_empty()).then(|| test_config_reason(&named));
        assert_eq!(&found, reason, "{paths:?}");
    }
}

// One scripted run: the status, exit code, duration and tails, and the clock advance.
type Script = (String, Option<i32>, f64, String, String, f64);

// The next scripted result for each call; the clock moves by the run's advance and only then.
pub(super) struct Scripted {
    clock: Arc<PinnedClock>,
    runs: Mutex<VecDeque<Script>>,
    calls: Mutex<Vec<Value>>,
}

impl Scripted {
    pub(super) fn new(clock: Arc<PinnedClock>, runs: Vec<Script>) -> Scripted {
        Scripted {
            clock,
            runs: Mutex::new(runs.into()),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Value> {
        self.calls
            .lock()
            .expect("no test panics with the lock")
            .clone()
    }

    fn left(&self) -> usize {
        self.runs
            .lock()
            .expect("no test panics with the lock")
            .len()
    }
}

// The status that displays as `name`.
fn status(name: &str) -> RunStatus {
    let found = STATUSES
        .into_iter()
        .find(|status| status.to_string() == name);
    found.unwrap_or_else(|| panic!("no run status {name}"))
}

// The text of app.py in the directory of a run, which shows the tree the run saw.
pub(super) fn probe(cwd: &Path) -> Option<String> {
    fs::read_to_string(cwd.join("app.py")).ok()
}

// A run that ended as `name` says, after `duration` seconds, with no output.
pub(super) fn result(name: &str, duration: f64) -> RunResult {
    RunResult {
        status: status(name),
        exit_code: (name == "passed").then_some(0),
        duration,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
    }
}

impl TestRunner for Scripted {
    fn run(&self, command: &TestCommand<'_>) -> Result<TestRun, TestRunError> {
        let call = json!([
            command.command,
            command.timeout.as_secs_f64(),
            probe(command.cwd)
        ]);
        self.calls
            .lock()
            .expect("no test panics with the lock")
            .push(call);
        let next = self
            .runs
            .lock()
            .expect("no test panics with the lock")
            .pop_front();
        let (name, exit_code, duration, stdout_tail, stderr_tail, advance) =
            next.expect("a scripted run for each call");
        self.clock.advance(Duration::from_secs_f64(advance));
        let result = RunResult {
            status: status(&name),
            exit_code,
            duration,
            stdout_tail,
            stderr_tail,
        };
        Ok(TestRun {
            result,
            notes: Vec::new(),
        })
    }

    fn interrupted(&self) -> bool {
        false
    }
}

#[derive(Debug, Deserialize)]
struct RequestRow {
    candidates: String,
    test_cmd: String,
    repro_cmd: Option<String>,
    timeout: i64,
    jobs: i64,
    repeat_baseline: i64,
    run_timeout: Option<i64>,
    pass_env: Vec<String>,
    test_cmd_from_repo: bool,
    allow_repo_test_cmd: bool,
    allow_test_config_edits: bool,
}

fn number(value: i64) -> IntArg {
    IntArg::parse(&value.to_string()).expect("an integer text")
}

impl RequestRow {
    fn request(&self, sandbox: &Path) -> ValidateRequest {
        ValidateRequest {
            candidates: sandbox.join(&self.candidates),
            test_cmd: self.test_cmd.clone(),
            repro_cmd: self.repro_cmd.clone(),
            timeout: number(self.timeout),
            jobs: number(self.jobs),
            repeat_baseline: number(self.repeat_baseline),
            run_timeout: self.run_timeout.map(number),
            pass_env: self.pass_env.clone(),
            test_cmd_from_repo: self.test_cmd_from_repo,
            allow_repo_test_cmd: self.allow_repo_test_cmd,
            allow_test_config_edits: self.allow_test_config_edits,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    steps: Vec<Vec<Value>>,
    root: String,
    request: RequestRow,
    runs: Vec<Script>,
    expected: Value,
    calls: Vec<Value>,
    diagnostics: Vec<String>,
}

// The services of `sandbox` with a monotonic clock that moves only when a test moves it.
pub(super) fn pinned(sandbox: &Sandbox) -> (Services, Arc<PinnedClock>) {
    let clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
    let mut services = sandbox.services(sandbox.path());
    services.clock = clock.clone();
    (services, clock)
}

// The report as the generator records it, or the refusal, with the diagnostics.
pub(super) fn validated(
    services: &Services,
    root: &Path,
    request: &ValidateRequest,
    runner: &dyn TestRunner,
) -> (Value, Vec<String>) {
    let repo = Repo::load(services, root).expect("the repository loads");
    let snapshot = services
        .git
        .snapshot(&repo.root, services.git.root(&repo.root).as_deref());
    let noted = validate(services, &repo, &snapshot, request, runner);
    let found = match noted.value {
        Ok(report) => json!({
            "jsonl": report.jsonl(RECEIPT),
            "summary": report.summary_line(),
            "warnings": report.warnings(),
            "any_passed": report.any_passed(),
        }),
        Err(error) => refusal(error),
    };
    (found, noted.diagnostics)
}

fn sorted(mut calls: Vec<Value>) -> Vec<Value> {
    calls.sort_by_cached_key(ToString::to_string);
    calls
}

fn scenario(case: &Scenario) {
    let sandbox = built(&case.steps);
    let (services, clock) = pinned(&sandbox);
    let runner = Scripted::new(clock, case.runs.clone());
    let request = case.request.request(sandbox.path());

    let (found, diagnostics) = validated(
        &services,
        &sandbox.path().join(&case.root),
        &request,
        &runner,
    );

    assert_eq!(shown(&sandbox, &found), case.expected, "{}", case.name);
    assert_eq!(runner.left(), 0, "{}: unused scripted runs", case.name);
    let calls = shown(&sandbox, &Value::Array(runner.calls()));
    let calls = calls.as_array().expect("a list").clone();
    if case.request.jobs == 1 {
        assert_eq!(calls, case.calls, "{} calls", case.name);
    } else {
        assert_eq!(
            sorted(calls),
            sorted(case.calls.clone()),
            "{} calls",
            case.name
        );
    }
    let diagnostics: Vec<String> = diagnostics
        .iter()
        .map(|line| masked(&sandbox, line))
        .collect();
    assert_eq!(diagnostics, case.diagnostics, "{} diagnostics", case.name);
    assert!(
        worktrees_left(&services).is_empty(),
        "{}: worktrees left",
        case.name
    );
}

// The scratch worktrees still on disk.
pub(super) fn worktrees_left(services: &Services) -> Vec<PathBuf> {
    let cache = services.cache_root.as_ref().expect("a cache root");
    let Ok(entries) = fs::read_dir(cache.path.join(git::WORKTREE_DIR)) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.expect("a directory entry").path())
        .collect()
}

#[test]
fn each_validate_case_gives_the_reference_document_calls_and_diagnostics() {
    let cases = buildable_cases("validate/service", "cases", |case: &Scenario| &case.steps);
    assert!(cases.len() >= 39, "{} cases", cases.len());
    cases.iter().for_each(scenario);
}
