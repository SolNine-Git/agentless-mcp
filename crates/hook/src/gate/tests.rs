use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use app::{EnvConfig, LintReport, Services};
use io::testing::cases;
use serde_json::Value;

use super::{Ports, run};
use crate::call::Call;
use crate::testing::{MANIFEST_DIR, Sandbox, Steps, latin};
use crate::worker::{LintFn, in_process};
use crate::{ENV_GATE, ENV_LOG, Output};

// One reference case: the name, the setup steps, the payload, the gate variable, the log form,
// and the expected code, stdout, stderr, compared stderr lines, and record line.
type Scenario = (
    String,
    Vec<Vec<Value>>,
    String,
    Option<String>,
    String,
    Value,
);

const BINARY: &str = "<BINARY>";
const GARBAGE: &str = "this file is not a SQLite database\n";

fn word(step: &[Value], index: usize) -> &str {
    step[index]
        .as_str()
        .unwrap_or_else(|| panic!("step {step:?}: no word {index}"))
}

fn apply(sandbox: &Sandbox, step: &[Value]) {
    match word(step, 0) {
        "init" => {
            sandbox.mkdir("repo");
            sandbox.git(&["init", "--quiet", "-b", "main"]);
        }
        "write" => drop(sandbox.put(word(step, 1), word(step, 2))),
        "bytes" => drop(sandbox.put(word(step, 1), latin(word(step, 2)))),
        "repeat" => {
            let count = step[3].as_u64().expect("a repeat count");
            let count = usize::try_from(count).expect("a count that fits");
            drop(sandbox.put(word(step, 1), word(step, 2).repeat(count)));
        }
        "commit" => {
            sandbox.git(&["add", "-A"]);
            sandbox.git(&["commit", "--quiet", "--allow-empty", "-m", "fixture"]);
        }
        "mkdir" => drop(sandbox.mkdir(word(step, 1))),
        "symlink" => sandbox.symlink(word(step, 1), word(step, 2)),
        "chmod" => {
            let mode = step[2].as_u64().expect("a mode");
            sandbox.set_mode(
                word(step, 1),
                u32::try_from(mode).expect("a mode that fits"),
            );
        }
        other => store_step(sandbox, other),
    }
}

// The cache steps: this build's index, a commit that leaves the index stale, and bytes that are no
// database in place of the index.
fn store_step(sandbox: &Sandbox, kind: &str) {
    match kind {
        "index" => indexed(sandbox),
        "stale" => {
            sandbox.put("repo/STALE.md", "stale\n");
            sandbox.git(&["add", "STALE.md"]);
            sandbox.git(&["commit", "--quiet", "-m", "fixture"]);
        }
        "cache_garbage" => drop(sandbox.put(&stored_database(sandbox), GARBAGE.repeat(128))),
        other => panic!("no scenario step {other}"),
    }
}

fn indexed(sandbox: &Sandbox) {
    let services = Services::new(&sandbox.env(&[]));
    let root = sandbox.root();
    let repo = app::Repo::load(&services, &root).expect("the repository loads");
    let snapshot = services
        .git
        .snapshot(&root, services.git.root(&root).as_deref());
    let report = app::index(&services, &repo, &snapshot, &app::IndexRequest::default()).value;
    report.unwrap_or_else(|error| panic!("the index step: {error}"));
}

// The path below the sandbox of the one database under its cache root.
fn stored_database(sandbox: &Sandbox) -> String {
    let cache = "home/.cache/agentless-mcp";
    let keys = std::fs::read_dir(sandbox.path().join(cache)).expect("the cache root is read");
    let found: Vec<String> = keys
        .map(|key| key.expect("a key directory").file_name())
        .map(|key| format!("{cache}/{}/agentless.db", key.to_string_lossy()))
        .filter(|database| sandbox.path().join(database).is_file())
        .collect();
    let [database] = found.as_slice() else {
        panic!("one database under the cache root: {found:?}");
    };
    database.clone()
}

fn log_path(sandbox: &Sandbox, form: &str) -> PathBuf {
    if form == "blocked" {
        sandbox.put("blocker", "a file, not a directory");
        return sandbox.path().join("blocker/erosion.jsonl");
    }
    sandbox.path().join("log/erosion.jsonl")
}

// Runs one call the way `erosion` does, with the fixed test clock and binary name.
fn call(sandbox: &Sandbox, gate: Option<&str>, log: &Path, lint: LintFn, payload: &[u8]) -> Output {
    let mut extra = vec![(ENV_LOG, log.as_os_str().to_owned())];
    extra.extend(gate.map(|gate| (ENV_GATE, OsString::from(gate))));
    let env = sandbox.env(&extra);
    let services = Services::new(&env);
    let mut stdin = payload;
    if gate == Some("off") {
        return crate::erosion(&env, &services, &mut stdin);
    }
    let call = Call::new(
        Box::new(Steps::fixed()),
        env.var(ENV_LOG).map(OsString::from),
    );
    let ports = Ports {
        services: &services,
        env_gate: env.var(ENV_GATE),
        lint,
        binary: Some(BINARY.to_owned()),
    };
    run(&call, &ports, &mut stdin)
}

fn first_lines(text: &str, count: &Value) -> String {
    let Some(count) = count.as_u64() else {
        return text.to_owned();
    };
    let count = usize::try_from(count).expect("a line count");
    text.lines().take(count).collect::<Vec<_>>().join("\n")
}

fn logged(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[test]
fn each_scenario_gives_the_reference_reply_and_record() {
    replay(cases_of("scenarios", 30));
}

// Writes one test for each listed shard, and the list that STATE_SHARDS counts.
macro_rules! state_shard_tests {
    ($($test:ident: $shard:literal),+ $(,)?) => {
        const SHARD_TESTS: &[usize] = &[$($shard),+];
        $(
            #[test]
            fn $test() {
                each_cache_state_of_shard($shard);
            }
        )+
    };
}

// The cache states: every scenario again on a fresh and on a stale cache, then the cases of the
// stored rows. Each test replays one shard, so that it ends well inside the slow-test timeout.
state_shard_tests! {
    each_cache_state_of_shard_0_gives_the_reference_reply_and_record: 0,
    each_cache_state_of_shard_1_gives_the_reference_reply_and_record: 1,
    each_cache_state_of_shard_2_gives_the_reference_reply_and_record: 2,
    each_cache_state_of_shard_3_gives_the_reference_reply_and_record: 3,
    each_cache_state_of_shard_4_gives_the_reference_reply_and_record: 4,
    each_cache_state_of_shard_5_gives_the_reference_reply_and_record: 5,
    each_cache_state_of_shard_6_gives_the_reference_reply_and_record: 6,
    each_cache_state_of_shard_7_gives_the_reference_reply_and_record: 7,
}

const STATE_SHARDS: usize = SHARD_TESTS.len();

// A repeated or a missing shard number leaves the cases of one shard unreplayed.
#[test]
fn the_shard_tests_replay_each_shard_once() {
    let expected: Vec<usize> = (0..STATE_SHARDS).collect();
    assert_eq!(SHARD_TESTS, expected.as_slice());
}

fn each_cache_state_of_shard(shard: usize) {
    assert!(shard < STATE_SHARDS, "shard {shard} of {STATE_SHARDS}");
    replay(
        cases_of("states", 150)
            .into_iter()
            .skip(shard)
            .step_by(STATE_SHARDS),
    );
}

fn cases_of(name: &str, more_than: usize) -> Vec<Scenario> {
    let all: Vec<Scenario> = cases(MANIFEST_DIR, name, name);
    assert!(all.len() > more_than, "{} {name}", all.len());
    all
}

// Replays each case and compares its reply and record.
fn replay(all: impl IntoIterator<Item = Scenario>) {
    for (name, steps, payload, gate, log_form, expected) in all {
        let sandbox = Sandbox::new();
        for step in &steps {
            apply(&sandbox, step);
        }
        let log = log_path(&sandbox, &log_form);
        let payload = latin(&sandbox.placed(&payload));
        let output = call(&sandbox, gate.as_deref(), &log, in_process, &payload);
        assert_eq!(Value::from(output.code), expected["code"], "{name}: code");
        assert_eq!(
            sandbox.placeholders(&output.stdout),
            expected["stdout"],
            "{name}: stdout"
        );
        let stderr = first_lines(
            &sandbox.placeholders(&output.stderr),
            &expected["stderr_lines"],
        );
        assert_eq!(stderr, expected["stderr"], "{name}: stderr");
        let record = logged(&log).map(|line| sandbox.placeholders(&line));
        assert_eq!(Value::from(record), expected["record"], "{name}: record");
    }
}

fn repo_with_file() -> Sandbox {
    let sandbox = Sandbox::new();
    sandbox.mkdir("repo");
    sandbox.git(&["init", "--quiet", "-b", "main"]);
    sandbox.put("repo/a.py", "x = 1\n");
    sandbox.git(&["add", "-A"]);
    sandbox.git(&["commit", "--quiet", "-m", "fixture"]);
    sandbox
}

fn write_payload(sandbox: &Sandbox) -> Vec<u8> {
    let path = sandbox.root().join("a.py");
    let payload = serde_json::json!({
        "session_id": "s1", "transcript_path": "/t.jsonl", "tool_use_id": "u1",
        "tool_name": "Write", "tool_input": {"file_path": path, "content": "x = 2\n"},
    });
    payload.to_string().into_bytes()
}

// Runs one Write with the given clock and lint, and returns the reply and the parsed record.
fn timed(offsets: &[Duration], lint: LintFn) -> (Output, Value) {
    let sandbox = repo_with_file();
    let log = sandbox.path().join("erosion.jsonl");
    let env = sandbox.env(&[]);
    let services = Services::new(&env);
    let call = Call::new(
        Box::new(Steps::new(offsets)),
        Some(log.clone().into_os_string()),
    );
    let ports = Ports {
        services: &services,
        env_gate: None,
        lint,
        binary: None,
    };
    let output = run(&call, &ports, &mut write_payload(&sandbox).as_slice());
    let text = logged(&log).expect("the call logged its record");
    (
        output,
        serde_json::from_str(&text).expect("the record is JSON"),
    )
}

// A lint that never returns: its thread parks until the test process exits.
const STUCK: LintFn = |_, _, _| {
    loop {
        thread::park();
    }
};

const NO_CANDIDATE: LintFn = |_, _, _| {
    Ok(LintReport {
        candidates: Vec::new(),
    })
};

const FAILING: LintFn =
    |_, _, _| Err(kernel::Error::OperationFailed("no cache root".to_owned()).into());

const PANICKING: LintFn = |_, _, _| panic!("a defect in lint");

const MS: fn(u64) -> Duration = Duration::from_millis;

#[test]
fn a_call_with_no_time_left_for_git_is_degraded() {
    let (output, record) = timed(&[MS(0), MS(9600)], in_process);
    assert_eq!(output, Output::default());
    assert_eq!(record["outcome"], "degraded");
    assert_eq!(record["reason"], "no time left for git");
    assert_eq!(record["root"], Value::Null);
}

#[test]
fn a_call_with_no_time_left_for_lint_is_degraded_after_the_digests() {
    let (_, record) = timed(&[MS(0), MS(0), MS(9600)], in_process);
    assert_eq!(record["reason"], "no time left for lint");
    assert!(record["new_sha256"].is_string(), "{record}");
    assert_eq!(record["checks"].as_array().map(Vec::len), Some(3));
}

#[test]
fn a_lint_that_outlives_the_deadline_is_degraded_without_a_wait_for_it() {
    let started = Instant::now();
    let (output, record) = timed(&[MS(0), MS(0), MS(9400)], STUCK);
    assert!(started.elapsed() < MS(2000), "{:?}", started.elapsed());
    assert_eq!(output, Output::default());
    assert_eq!(record["reason"], "lint timed out after 0.1 s");
}

#[test]
fn a_lint_failure_is_degraded_with_its_error() {
    let (output, record) = timed(&[MS(0)], FAILING);
    assert_eq!(output, Output::default());
    assert_eq!(record["outcome"], "degraded");
    assert_eq!(record["reason"], "lint failed: no cache root");
}

#[test]
fn a_panic_in_lint_is_an_internal_error_that_exits_0() {
    let (output, record) = timed(&[MS(0)], PANICKING);
    assert_eq!(output.code, 0);
    assert_eq!(output.stdout, "");
    let path = record["path"].as_str().expect("the path is set");
    let expected = format!(
        "erosion gate internal error: tool_use_id=u1 path={path}\npanic: a defect in lint\n"
    );
    assert_eq!(output.stderr, expected);
    assert_eq!(record["reason"], "internal error: panic: a defect in lint");
    assert_eq!(record["dropped"], serde_json::json!([]));
}

#[test]
fn a_lint_with_no_candidate_is_an_internal_error() {
    let (output, record) = timed(&[MS(0)], NO_CANDIDATE);
    assert_eq!((output.code, output.stdout.as_str()), (0, ""));
    let path = record["path"].as_str().expect("the path is set");
    let expected = format!(
        "erosion gate internal error: tool_use_id=u1 path={path}\nlint returned 0 candidates, \
         not one\n"
    );
    assert_eq!(output.stderr, expected);
    assert_eq!(
        record["reason"],
        "internal error: lint returned 0 candidates, not one"
    );
}

#[test]
fn git_absent_from_path_degrades_with_the_cannot_run_git_text() {
    let sandbox = repo_with_file();
    let empty = sandbox.mkdir("empty-bin");
    let log = sandbox.path().join("erosion.jsonl");
    let mut vars: Vec<(OsString, OsString)> = sandbox.env(&[]).vars().to_vec();
    vars.retain(|(name, _)| name != "PATH");
    vars.push((OsString::from("PATH"), empty.into_os_string()));
    let env = EnvConfig::new(vars, sandbox.path().to_path_buf());
    let services = Services::new(&env);
    let call = Call::new(Box::new(Steps::fixed()), Some(log.clone().into_os_string()));
    let ports = Ports {
        services: &services,
        env_gate: None,
        lint: in_process,
        binary: None,
    };

    let output = run(&call, &ports, &mut write_payload(&sandbox).as_slice());

    assert_eq!(output, Output::default());
    let record: Value =
        serde_json::from_str(&logged(&log).expect("a record")).expect("the record is JSON");
    assert_eq!(record["outcome"], "degraded");
    assert_eq!(
        record["reason"],
        "cannot run git: [Errno 2] No such file or directory: 'git'"
    );
}

#[test]
fn the_public_entry_logs_its_own_binary() {
    let sandbox = repo_with_file();
    let log = sandbox.path().join("erosion.jsonl");
    let env = sandbox.env(&[(ENV_LOG, log.clone().into_os_string())]);
    let services = Services::new(&env);
    let output = crate::erosion(&env, &services, &mut write_payload(&sandbox).as_slice());
    assert_eq!(output.code, 0, "{output:?}");
    let record: Value =
        serde_json::from_str(&logged(&log).expect("a record")).expect("the record is JSON");
    let binary = std::env::current_exe().expect("the test binary has a path");
    assert_eq!(record["binary"], binary.display().to_string());
    assert!(record["latency_ms"].as_f64().is_some(), "{record}");
}
