use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread::{self, ThreadId};
use std::time::Duration;

use io::{GroupNote, RelativeCacheHome, TestCommand, TestRun, TestRunError};
use serde_json::{Value, json};

use super::{pinned, probe, result, validated, worktrees_left};
use crate::testing::Sandbox;
use crate::validate::{EnvRunner, INTERRUPTED, TestRunner, ValidateRequest};
use crate::{IntArg, Services};

const APP: &str = "def add(a, b):\n    return a - b\n";

fn block(replace: &str) -> String {
    let search = "### app.py\n<<<<<<< SEARCH\n    return a - b\n=======\n";
    format!("{search}    return {replace}\n>>>>>>> REPLACE\n")
}

// A committed repository with app.py, and one candidate file per `(name, replacement)`.
fn setup(candidates: &[(&str, &str)]) -> (Sandbox, PathBuf) {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", APP)]);
    for (name, replace) in candidates {
        sandbox.put(&format!("cands/{name}"), block(replace));
    }
    (sandbox, root)
}

fn request(sandbox: &Sandbox, jobs: usize) -> ValidateRequest {
    ValidateRequest {
        candidates: sandbox.path().join("cands"),
        test_cmd: "run-tests".to_owned(),
        repro_cmd: None,
        timeout: IntArg::from_usize(300),
        jobs: IntArg::from_usize(jobs),
        repeat_baseline: IntArg::from_usize(1),
        run_timeout: None,
        pass_env: Vec::new(),
        test_cmd_from_repo: false,
        allow_repo_test_cmd: false,
        allow_test_config_edits: false,
    }
}

type Answer = dyn Fn(usize, Option<&str>) -> Result<TestRun, TestRunError> + Sync;

// A runner that answers each call from its number and the app.py it sees, and that reports a
// SIGINT once `interrupt_after` runs have ended.
struct Answering {
    answer: Box<Answer>,
    interrupt_after: usize,
    ran: AtomicUsize,
}

impl Answering {
    fn new(answer: Box<Answer>) -> Answering {
        Answering {
            answer,
            interrupt_after: usize::MAX,
            ran: AtomicUsize::new(0),
        }
    }

    fn ran(&self) -> usize {
        self.ran.load(Ordering::SeqCst)
    }
}

impl TestRunner for Answering {
    fn run(&self, command: &TestCommand<'_>) -> Result<TestRun, TestRunError> {
        let call = self.ran.fetch_add(1, Ordering::SeqCst);
        (self.answer)(call, probe(command.cwd).as_deref())
    }

    fn interrupted(&self) -> bool {
        self.ran() >= self.interrupt_after
    }
}

fn passed(notes: Vec<GroupNote>) -> TestRun {
    TestRun {
        result: result("passed", 0.5),
        notes,
    }
}

fn stopped_message(found: &Value) -> Option<&str> {
    let error = found.get("error")?.as_array()?;
    if error.first()?.as_str()? != "CallStopped" {
        return None;
    }
    error.get(1)?.as_str()
}

#[test]
fn an_interrupt_during_a_run_stops_the_call_after_the_group_notes() {
    let (sandbox, root) = setup(&[("01-plus.txt", "a + b"), ("02-wrong.txt", "a * b")]);
    let (services, _clock) = pinned(&sandbox);
    let runner = Answering::new(Box::new(|call: usize, _: Option<&str>| {
        if call == 0 {
            return Ok(passed(Vec::new()));
        }
        Err(TestRunError::Interrupted {
            notes: vec![GroupNote::NotPermitted { group: 41 }],
        })
    }));

    let (found, diagnostics) = validated(&services, &root, &request(&sandbox, 1), &runner);

    assert_eq!(stopped_message(&found), Some(INTERRUPTED), "{found}");
    assert_eq!(
        runner.ran(),
        2,
        "no representative runs after the interrupt"
    );
    let note = "not permitted to signal process group 41; it may survive this run";
    assert_eq!(diagnostics, [note]);
    assert!(worktrees_left(&services).is_empty());
}

#[test]
fn an_interrupt_seen_after_a_run_starts_no_further_representative() {
    let candidates = [
        ("01-plus.txt", "a + b"),
        ("02-wrong.txt", "a * b"),
        ("03-div.txt", "a / b"),
    ];
    let (sandbox, root) = setup(&candidates);
    let (services, _clock) = pinned(&sandbox);
    let mut runner = Answering::new(Box::new(|_: usize, _: Option<&str>| Ok(passed(Vec::new()))));
    runner.interrupt_after = 2;

    let (found, _) = validated(&services, &root, &request(&sandbox, 1), &runner);

    assert_eq!(stopped_message(&found), Some(INTERRUPTED), "{found}");
    assert_eq!(runner.ran(), 2);
    assert!(worktrees_left(&services).is_empty());
}

fn no_space() -> std::io::Error {
    std::io::Error::from_raw_os_error(28)
}

#[test]
fn a_capture_failure_is_an_operation_failure_after_the_group_notes() {
    let (sandbox, root) = setup(&[("01-plus.txt", "a + b")]);
    let (services, _clock) = pinned(&sandbox);
    let runner = Answering::new(Box::new(|_: usize, _: Option<&str>| {
        Err(TestRunError::Capture {
            error: no_space(),
            notes: vec![GroupNote::StillPresent { group: 9 }],
        })
    }));
    let expected = TestRunError::Capture {
        error: no_space(),
        notes: Vec::new(),
    };

    let (found, diagnostics) = validated(&services, &root, &request(&sandbox, 1), &runner);

    assert_eq!(
        found,
        json!({"error": ["OperationFailed", expected.to_string()]})
    );
    let note = "process group 9 is still present after SIGKILL; it may be stuck in the kernel";
    assert_eq!(diagnostics, [note]);
    assert!(worktrees_left(&services).is_empty());
}

#[test]
fn the_group_notes_of_each_run_come_in_run_order() {
    let (sandbox, root) = setup(&[("01-plus.txt", "a + b"), ("02-wrong.txt", "a * b")]);
    let (services, _clock) = pinned(&sandbox);
    let runner = Answering::new(Box::new(|call: usize, _: Option<&str>| {
        let group = u32::try_from(call).expect("a small call number");
        Ok(passed(vec![GroupNote::NotPermitted { group }]))
    }));

    let (found, diagnostics) = validated(&services, &root, &request(&sandbox, 1), &runner);

    assert!(found.get("jsonl").is_some(), "{found}");
    let groups: Vec<String> = (0..3)
        .map(|group| GroupNote::NotPermitted { group }.to_string())
        .collect();
    assert_eq!(diagnostics, groups);
}

#[test]
fn an_ignored_cache_home_is_warned_once_before_the_first_worktree() {
    let (sandbox, root) = setup(&[("01-plus.txt", "a + b"), ("02-wrong.txt", "a * b")]);
    let (mut services, _clock) = pinned(&sandbox);
    let ignored = RelativeCacheHome(OsString::from("relcache"));
    let warning = ignored.to_string();
    if let Ok(cache) = services.cache_root.as_mut() {
        cache.ignored = Some(ignored);
    }
    let runner = Answering::new(Box::new(|_: usize, _: Option<&str>| Ok(passed(Vec::new()))));

    let (found, diagnostics) = validated(&services, &root, &request(&sandbox, 1), &runner);

    assert!(found.get("jsonl").is_some(), "{found}");
    assert_eq!(runner.ran(), 3, "three worktrees ran a command");
    assert_eq!(diagnostics, [warning]);
}

#[test]
fn a_cache_root_that_is_not_utf8_still_holds_the_worktrees() {
    // APFS refuses a file name that is not UTF-8.
    if cfg!(target_os = "macos") {
        return;
    }
    let (sandbox, root) = setup(&[("01-plus.txt", "a + b")]);
    let (mut services, _clock) = pinned(&sandbox);
    if let Ok(cache) = services.cache_root.as_mut() {
        cache.path = sandbox.path().join(OsStr::from_bytes(b"cache-\xff"));
    }
    let runner = Answering::new(Box::new(|_: usize, _: Option<&str>| Ok(passed(Vec::new()))));

    let (found, diagnostics) = validated(&services, &root, &request(&sandbox, 1), &runner);

    assert!(found.get("jsonl").is_some(), "{found}");
    assert_eq!(runner.ran(), 2, "the baseline and the candidate ran");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert!(worktrees_left(&services).is_empty());
}

// Passes the unchanged tree and a fix, and fails anything else, whatever the order of the calls.
fn by_content(probe: Option<&str>) -> TestRun {
    let fixed = probe.is_some_and(|text| text.contains("a + b") || text.contains("a+b"));
    let name = if fixed || probe == Some(APP) {
        "passed"
    } else {
        "failed"
    };
    TestRun {
        result: result(name, 0.25),
        notes: Vec::new(),
    }
}

fn candidate_records(services: &Services, root: &Path, request: &ValidateRequest) -> Vec<String> {
    let runner = Answering::new(Box::new(|_: usize, probe: Option<&str>| {
        Ok(by_content(probe))
    }));
    let (found, _) = validated(services, root, request, &runner);
    let document = found["jsonl"].as_str().expect("a document");
    document.lines().skip(1).map(str::to_owned).collect()
}

#[test]
fn two_jobs_give_the_candidate_records_of_one_job() {
    let candidates = [
        ("01-plus.txt", "a + b"),
        ("02-again.txt", "a + b"),
        ("03-spaced.txt", "a+b"),
        ("04-wrong.txt", "a * b"),
        ("05-div.txt", "a / b"),
    ];
    let (sandbox, root) = setup(&candidates);
    let (services, _clock) = pinned(&sandbox);

    let serial = candidate_records(&services, &root, &request(&sandbox, 1));
    let parallel = candidate_records(&services, &root, &request(&sandbox, 2));

    assert_eq!(serial.len(), 5);
    assert_eq!(parallel, serial);
    assert!(worktrees_left(&services).is_empty());
}

// Holds each candidate run on the calling thread until a helper thread has panicked, so the
// panic is a helper's. The wait is bounded: with no helper, the call ends without a panic.
struct HelperPanics {
    caller: ThreadId,
    panicked: Mutex<bool>,
    woken: Condvar,
}

impl TestRunner for HelperPanics {
    fn run(&self, command: &TestCommand<'_>) -> Result<TestRun, TestRunError> {
        if probe(command.cwd).as_deref() == Some(APP) {
            return Ok(passed(Vec::new()));
        }
        if thread::current().id() != self.caller {
            *self.panicked.lock().expect("no panic with the lock") = true;
            self.woken.notify_all();
            panic!("the scripted panic on a helper thread");
        }
        let flag = self.panicked.lock().expect("no panic with the lock");
        let waited = self
            .woken
            .wait_timeout_while(flag, Duration::from_secs(5), |done| !*done);
        drop(waited.expect("no panic with the lock"));
        Ok(passed(Vec::new()))
    }

    fn interrupted(&self) -> bool {
        false
    }
}

#[test]
fn a_panic_in_a_run_on_a_helper_thread_reaches_the_caller_and_releases_the_worktrees() {
    let candidates = [
        ("01-plus.txt", "a + b"),
        ("02-wrong.txt", "a * b"),
        ("03-div.txt", "a / b"),
    ];
    let (sandbox, root) = setup(&candidates);
    let (services, _clock) = pinned(&sandbox);
    let runner = HelperPanics {
        caller: thread::current().id(),
        panicked: Mutex::new(false),
        woken: Condvar::new(),
    };
    let jobs = request(&sandbox, 2);

    let caught = panic::catch_unwind(AssertUnwindSafe(|| {
        validated(&services, &root, &jobs, &runner)
    }));

    let payload = caught.expect_err("the panic reaches the caller");
    let message = payload.downcast_ref::<&str>().copied().unwrap_or_default();
    assert_eq!(message, "the scripted panic on a helper thread");
    assert!(worktrees_left(&services).is_empty());
}

#[test]
fn the_environment_runner_runs_a_command_with_the_start_up_variables() {
    let sandbox = Sandbox::new();
    let env = sandbox.env(sandbox.path());
    let runner = EnvRunner::new(&env);
    let command = TestCommand {
        cwd: sandbox.path(),
        command: "sh -c 'exit 3'",
        timeout: Duration::from_secs(30),
        max_capture: io::DEFAULT_MAX_CAPTURE,
        pass_env: &[],
    };

    let ran = runner.run(&command).expect("a finished run");

    assert_eq!(ran.result.status, io::RunStatus::Failed);
    assert_eq!(ran.result.exit_code, Some(3));
    assert!(!runner.interrupted());
}
