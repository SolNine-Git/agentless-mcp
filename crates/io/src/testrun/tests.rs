use std::collections::BTreeMap;
use std::fs;
use std::thread;

use serde::Deserialize;
use signal_hook::consts::SIGINT;

use super::*;
use crate::fixtures::{fixture, is_child, run_child, tree};
use crate::testing::Tree;

const LONG: Duration = Duration::from_secs(30);
const QUICK: Bounds = Bounds {
    poll: Duration::from_millis(50),
    grace: Duration::from_millis(300),
    reap: Duration::from_millis(300),
};

// A script that records its group and a child that ignores SIGTERM, then outlives any bound. A
// kill of the leader alone leaves that child running.
const HANG: &str = "sh -c 'trap \"\" TERM; exec sleep 600' &\n\
                    echo \"$$ $!\" > \"$1\"\n\
                    exec sleep 600\n";

// The process environment of a test: PATH to find the programs, and the capture directory.
fn env_for(tree: &Tree) -> impl Fn(&str) -> Option<OsString> {
    let mut vars = BTreeMap::new();
    vars.insert(
        "PATH",
        std::env::var_os("PATH").expect("the tests find sh through PATH"),
    );
    vars.insert("TMPDIR", tree.path().as_os_str().to_owned());
    move |name: &str| vars.get(name).cloned()
}

fn command<'a>(tree: &'a Tree, line: &'a str, timeout: Duration) -> TestCommand<'a> {
    TestCommand {
        cwd: tree.path(),
        command: line,
        timeout,
        max_capture: DEFAULT_MAX_CAPTURE,
        pass_env: &[],
    }
}

fn ran(
    command: &TestCommand<'_>,
    env: &impl Fn(&str) -> Option<OsString>,
    bounds: Bounds,
) -> RunResult {
    match run_bounded(command, env, bounds) {
        Ok(run) => {
            assert!(run.notes.is_empty(), "{:?}", run.notes);
            run.result
        }
        Err(error) => panic!("the run gave no result: {error}"),
    }
}

fn run(tree: &Tree, line: &str) -> RunResult {
    ran(&command(tree, line, LONG), &env_for(tree), BOUNDS)
}

#[test]
fn a_zero_exit_is_a_pass() {
    let tree = tree();
    let result = run(&tree, "sh -c 'echo fine'");
    assert_eq!(
        (result.status, result.exit_code),
        (RunStatus::Passed, Some(0))
    );
    assert!(result.passed());
    assert_eq!(result.stdout_tail, "fine\n");
}

#[test]
fn a_non_zero_exit_is_a_failure_that_carries_stderr() {
    let tree = tree();
    let result = run(&tree, "sh -c 'echo the suite is red >&2; exit 3'");
    assert_eq!(
        (result.status, result.exit_code),
        (RunStatus::Failed, Some(3))
    );
    assert!(!result.passed());
    assert_eq!(result.stderr_tail, "the suite is red\n");
}

#[test]
fn a_signal_exit_is_a_failure_with_the_negative_signal() {
    let tree = tree();
    let result = run(&tree, "sh -c 'kill -9 $$'");
    assert_eq!(
        (result.status, result.exit_code),
        (RunStatus::Failed, Some(-9))
    );
}

#[test]
fn the_command_runs_in_the_directory_it_was_given() {
    let tree = tree();
    let result = run(&tree, "pwd");
    assert_eq!(Path::new(result.stdout_tail.trim_end()), tree.path());
}

#[test]
fn only_the_allowed_and_the_passed_names_reach_the_command() {
    let tree = tree();
    let mut vars = BTreeMap::new();
    vars.insert("PATH", std::env::var_os("PATH").expect("a PATH"));
    vars.insert("HOME", OsString::from("/home/somebody"));
    vars.insert(
        "AGENTLESS_MCP_TEST_SECRET",
        OsString::from("contained-value"),
    );
    let env = |name: &str| vars.get(name).cloned();
    let line = "sh -c 'echo \"${AGENTLESS_MCP_TEST_SECRET-absent} ${HOME-unset} ${LANG-unset}\"'";
    let scrubbed = ran(&command(&tree, line, LONG), &env, BOUNDS);
    let pass = ["AGENTLESS_MCP_TEST_SECRET".to_owned()];
    let passed = TestCommand {
        pass_env: &pass,
        ..command(&tree, line, LONG)
    };
    assert_eq!(scrubbed.stdout_tail, "absent /home/somebody unset\n");
    assert_eq!(
        ran(&passed, &env, BOUNDS).stdout_tail,
        "contained-value /home/somebody unset\n"
    );
}

#[test]
fn the_environment_holds_each_present_name_once() {
    let vars: BTreeMap<&str, &str> = [("PATH", "/bin"), ("LANG", "C"), ("EXTRA", "1")].into();
    let env = |name: &str| vars.get(name).map(OsString::from);
    let pass = ["EXTRA".to_owned(), "PATH".to_owned(), "MISSING".to_owned()];
    let built = environment(&pass, &env);
    let names: Vec<&str> = built.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["PATH", "LANG", "EXTRA"]);
    assert_eq!(TEST_ENV_ALLOWLIST, ["PATH", "HOME", "LANG", "TMPDIR"]);
}

// The child's environment is in name order, not in the order the names were given.
#[test]
fn the_child_sees_its_variables_in_name_order() {
    let tree = tree();
    let path = std::env::var_os("PATH").expect("the tests find env through PATH");
    let vars: BTreeMap<&str, OsString> = [
        ("PATH", path),
        ("ZZZ", OsString::from("1")),
        ("AAA", OsString::from("2")),
    ]
    .into();
    let env = |name: &str| vars.get(name).cloned();
    let pass = ["ZZZ".to_owned(), "AAA".to_owned()];
    let listed = TestCommand {
        pass_env: &pass,
        ..command(&tree, "env", LONG)
    };
    let result = ran(&listed, &env, BOUNDS);
    let names: Vec<&str> = result
        .stdout_tail
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name))
        .collect();
    assert_eq!(names, ["AAA", "PATH", "ZZZ"]);
}

// The spawn's own error names an empty program: no PATH entry is tried.
#[test]
fn an_empty_program_is_a_start_failure_with_the_spawn_error() {
    let tree = tree();
    let result = run(&tree, "\"\"");
    assert_eq!(
        (result.status, result.stderr_tail.as_str()),
        (
            RunStatus::Error,
            "could not start '': No such file or directory"
        )
    );
}

#[test]
fn the_captures_try_tmpdir_then_tmp() {
    let dirs = |value: Option<&str>| capture_dirs(&|_: &str| value.map(OsString::from));
    let tmp = || PathBuf::from("/tmp");
    assert_eq!(dirs(None), [tmp()]);
    assert_eq!(dirs(Some("")), [tmp()]);
    assert_eq!(dirs(Some("/tmp")), [tmp()]);
    assert_eq!(dirs(Some("/scratch")), [PathBuf::from("/scratch"), tmp()]);
}

#[test]
fn an_unusable_tmpdir_puts_the_captures_in_tmp() {
    let tree = tree();
    let file = tree.write_file("plain", "x").expect("the file is written");
    let read_only = tree
        .create_dir("read-only")
        .expect("the directory is created");
    tree.chmod("read-only", 0o555);
    let mut unusable = vec![tree.join("missing"), file];
    if tree.modes_bind() {
        unusable.push(read_only);
    }
    for tmpdir in unusable {
        let path = std::env::var_os("PATH").expect("the tests find sh through PATH");
        let env = |name: &str| match name {
            "PATH" => Some(path.clone()),
            "TMPDIR" => Some(tmpdir.clone().into_os_string()),
            _ => None,
        };
        let result = ran(&command(&tree, "echo ok", LONG), &env, BOUNDS);
        assert_eq!(result.status, RunStatus::Passed, "{}", tmpdir.display());
        assert_eq!(result.stdout_tail, "ok\n", "{}", tmpdir.display());
    }
    tree.chmod("read-only", 0o755);
}

#[test]
fn no_capture_directory_names_each_directory_and_its_error() {
    let tree = tree();
    let file = tree.write_file("plain", "x").expect("the file is written");
    let missing = tree.join("missing");
    let refused = Captures::create_in(&[missing.clone(), file.clone()]).err();
    let refused = refused.expect("neither directory takes the files");
    let shown = TestRunError::NoCaptureDir(refused).to_string();
    assert_eq!(
        shown,
        format!(
            "a capture file of the test run failed: FileNotFoundError: [Errno 2] No such file or \
             directory: '{}'; NotADirectoryError: [Errno 20] Not a directory: '{}'",
            missing.display(),
            file.display()
        )
    );
}

#[test]
fn the_bounds_are_the_documented_values() {
    assert_eq!(
        (
            CAPTURE_POLL,
            TERM_GRACE,
            KILL_REAP,
            CAPTURE_SLACK,
            DEFAULT_MAX_CAPTURE
        ),
        (
            Duration::from_millis(250),
            Duration::from_secs(5),
            Duration::from_secs(1),
            8,
            100_000
        )
    );
}

fn recorded(marker: &Path) -> Option<(u32, u32)> {
    let text = fs::read_to_string(marker).ok()?;
    let mut fields = text.split_whitespace().map(|field| field.parse().ok());
    Some((fields.next()??, fields.next()??))
}

// Polls until `pid` has exited; another owner means the pid was reaped and reused.
fn process_is_gone(pid: u32) -> bool {
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .expect("a child pid");
    let stop = Instant::now() + Duration::from_secs(15);
    while Instant::now() < stop {
        match rustix::process::test_kill_process(pid) {
            Err(error) if error == Errno::SRCH || error == Errno::PERM => return true,
            _ => thread::sleep(Duration::from_millis(50)),
        }
    }
    false
}

#[test]
fn a_hang_is_a_timeout_and_the_whole_group_is_dead_afterwards() {
    let tree = tree();
    tree.write_file("hang.sh", HANG)
        .expect("the script is written");
    let marker = tree.join("pgid.txt");
    let line = format!("sh hang.sh {}", marker.display());
    let started = Instant::now();
    let result = ran(
        &command(&tree, &line, Duration::from_secs(1)),
        &env_for(&tree),
        BOUNDS,
    );
    let elapsed = started.elapsed();
    assert_eq!(
        (result.status, result.exit_code),
        (RunStatus::Timeout, None)
    );
    assert!(!result.passed());
    assert!(
        elapsed < Duration::from_secs(1) + TERM_GRACE + KILL_REAP + LONG,
        "{elapsed:?}"
    );
    let (group, child) = recorded(&marker).expect("hang.sh recorded its group and its child");
    assert!(group > 1);
    assert!(
        process_is_gone(child),
        "child process {child} outlived the run"
    );
}

#[test]
fn a_leader_that_ignores_sigterm_costs_the_grace_and_the_reap_at_most() {
    let tree = tree();
    let line = "sh -c 'trap \"\" TERM; exec sleep 600'";
    let timeout = Duration::from_millis(500);
    let started = Instant::now();
    let result = ran(&command(&tree, line, timeout), &env_for(&tree), QUICK);
    let elapsed = started.elapsed();
    assert_eq!(result.status, RunStatus::Timeout);
    assert!(
        elapsed >= timeout + QUICK.grace,
        "the grace was not waited: {elapsed:?}"
    );
    let bound = timeout + QUICK.grace + QUICK.reap + Duration::from_secs(3);
    assert!(elapsed < bound, "{elapsed:?}");
}

#[test]
fn an_interrupted_run_still_ends_the_process_group() {
    let (status, stderr) = run_child("testrun::tests::child_interrupts_a_hanging_run");
    assert_eq!(status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("the group is gone"), "{stderr}");
}

#[test]
fn child_interrupts_a_hanging_run() {
    if !is_child("testrun::tests::child_interrupts_a_hanging_run") {
        return;
    }
    let tree = tree();
    tree.write_file("hang.sh", HANG)
        .expect("the script is written");
    let marker = tree.join("pgid.txt");
    let line = format!("sh hang.sh {}", marker.display());
    let watcher = marker.clone();
    let interrupter = thread::spawn(move || {
        let stop = Instant::now() + LONG;
        while recorded(&watcher).is_none() && Instant::now() < stop {
            thread::sleep(Duration::from_millis(20));
        }
        signal_hook::low_level::raise(SIGINT).expect("SIGINT is raised");
    });
    let outcome = run_bounded(&command(&tree, &line, LONG), &env_for(&tree), BOUNDS);
    interrupter.join().expect("the interrupter returns");
    assert!(
        matches!(outcome, Err(TestRunError::Interrupted { .. })),
        "{outcome:?}"
    );
    assert!(crate::interrupted());
    let (_, child) = recorded(&marker).expect("hang.sh recorded its group and its child");
    assert!(
        process_is_gone(child),
        "child process {child} outlived the interrupt"
    );
    eprintln!("the group is gone");
}

// As an unwind does: the run is dropped while its group still runs.
#[test]
fn a_run_dropped_before_its_wait_ends_still_kills_the_group() {
    let tree = tree();
    tree.write_file("hang.sh", HANG)
        .expect("the script is written");
    let marker = tree.join("pgid.txt");
    let args = ["hang.sh".to_owned(), marker.display().to_string()];
    let line = format!("sh hang.sh {}", marker.display());
    let test = command(&tree, &line, LONG);
    let captures = Captures::create(tree.path()).expect("the captures are created");
    let spawned = spawn(&test, "sh", &args, &env_for(&tree), &captures);
    let child = spawned.expect("the captures pass").expect("sh starts");
    let running = Running {
        child,
        captures,
        hold: hold_sigint().expect("the watch registers"),
        started: Instant::now(),
        bounds: QUICK,
        ended: false,
    };
    let stop = Instant::now() + LONG;
    while recorded(&marker).is_none() && Instant::now() < stop {
        thread::sleep(Duration::from_millis(20));
    }
    drop(running);
    let (_, child) = recorded(&marker).expect("hang.sh recorded its group and its child");
    assert!(
        process_is_gone(child),
        "child process {child} outlived the run"
    );
}

#[test]
fn the_wait_for_a_running_child_lasts_its_whole_bound() {
    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("sleep starts");
    let within = Duration::from_millis(50);
    let started = Instant::now();
    let gone = reaped(&mut child, within);
    let waited = started.elapsed();
    child.kill().expect("the child is killed");
    child.wait().expect("the child is reaped");
    assert!(!gone, "the running child counted as reaped");
    assert!(waited >= within, "the wait ended early: {waited:?}");
}

#[test]
fn ending_an_already_reaped_group_gives_no_note() {
    let mut child = Command::new("true").spawn().expect("true starts");
    child.wait().expect("true is reaped");
    assert_eq!(end_group(&mut child, QUICK), []);
}

#[test]
fn each_kill_problem_has_its_line() {
    let refused = kill_notes(4_000_000, Err(Errno::PERM), Err(Errno::SRCH), true);
    assert_eq!(refused, [GroupNote::NotPermitted { group: 4_000_000 }]);
    assert_eq!(
        refused[0].to_string(),
        "not permitted to signal process group 4000000; it may survive this run"
    );
    let stuck = kill_notes(42, Ok(()), Ok(()), false);
    assert_eq!(
        stuck.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["process group 42 is still present after SIGKILL; it may be stuck in the kernel"]
    );
    let other = kill_notes(42, Err(Errno::INVAL), Ok(()), true);
    assert_eq!(
        other[0].to_string(),
        "could not signal process group 42: Invalid argument"
    );
    assert_eq!(kill_notes(42, Err(Errno::SRCH), Err(Errno::SRCH), true), []);
}

#[test]
fn each_run_error_has_its_text() {
    let errno = |code: i32| io::Error::from_raw_os_error(code);
    let errors = [
        TestRunError::Interrupted { notes: Vec::new() },
        TestRunError::Capture {
            error: errno(28),
            notes: Vec::new(),
        },
        TestRunError::Wait {
            error: errno(10),
            notes: Vec::new(),
        },
        TestRunError::Watch(crate::WatchError(errno(22))),
    ];
    let shown: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        shown,
        [
            "the test run was interrupted by SIGINT",
            "a capture file of the test run failed: OSError: [Errno 28] No space left on device",
            "the wait for the test command failed: ChildProcessError: [Errno 10] No child processes",
            "SIGINT cannot be watched: OSError: [Errno 22] Invalid argument",
        ]
    );
}

#[test]
fn the_group_of_pid_1_is_never_signalled() {
    assert_eq!(signal_group(1, Signal::TERM), Err(Errno::SRCH));
    assert_eq!(signal_group(0, Signal::TERM), Err(Errno::SRCH));
}

#[test]
fn the_capture_keeps_the_tail() {
    let tree = tree();
    let line =
        "sh -c 'i=0; while [ $i -lt 2000 ]; do printf \"line %05d\\n\" $i; i=$((i+1)); done'";
    let small = TestCommand {
        max_capture: 200,
        ..command(&tree, line, LONG)
    };
    let result = ran(&small, &env_for(&tree), BOUNDS);
    assert_eq!(result.status, RunStatus::Passed);
    assert!(
        result
            .stdout_tail
            .starts_with("[... 21800 earlier bytes dropped ...]\n"),
        "{}",
        result.stdout_tail
    );
    assert!(result.stdout_tail.ends_with("line 01999\n"));
    assert!(!result.stdout_tail.contains("line 00000"));
}

#[test]
fn a_short_output_is_not_marked_as_dropped() {
    let tree = tree();
    let small = TestCommand {
        max_capture: 200,
        ..command(&tree, "sh -c 'echo brief'", LONG)
    };
    assert_eq!(ran(&small, &env_for(&tree), BOUNDS).stdout_tail, "brief\n");
}

// Two megabytes, a pause that spans several polls, the size the capture file holds, a marker.
const FLOOD: &str = "exec 3>&1\n\
                     i=0; while [ $i -lt 256 ]; do printf '%8192s' ''; i=$((i+1)); done\n\
                     sleep 1\n\
                     held=$(stat -L -c %s /dev/fd/3 2>/dev/null || stat -L -f %z /dev/fd/3)\n\
                     echo \"held=$held\" >&2\n\
                     echo tail-marker\n";

#[test]
fn the_capture_is_bounded_while_it_is_written_and_its_tail_is_output() {
    let tree = tree();
    tree.write_file("flood.sh", FLOOD)
        .expect("the script is written");
    let small = TestCommand {
        max_capture: 1000,
        ..command(&tree, "sh flood.sh", LONG)
    };
    let result = ran(&small, &env_for(&tree), BOUNDS);
    assert_eq!(result.status, RunStatus::Passed, "{}", result.stderr_tail);
    let held: u64 = result
        .stderr_tail
        .trim_end()
        .strip_prefix("held=")
        .and_then(|size| size.parse().ok())
        .unwrap_or_else(|| panic!("no size: {}", result.stderr_tail));
    assert!(
        held <= 1000 * CAPTURE_SLACK,
        "the command left {held} bytes on disk"
    );
    let dropped = 256 * 8192 + "tail-marker\n".len() - 1000;
    let expected = format!("[... {dropped} earlier bytes dropped ...]\ntail-marker\n");
    assert_eq!(result.stdout_tail, expected);
}

#[test]
fn the_command_is_never_interpreted_by_a_shell() {
    let tree = tree();
    let canary = tree.join("canary.txt");
    let line = format!("sh -c 'echo $#' zero ; touch {}", canary.display());
    let result = run(&tree, &line);
    assert_eq!(
        (result.status, result.stdout_tail.as_str()),
        (RunStatus::Passed, "3\n")
    );
    assert!(!canary.exists());
}

#[test]
fn stdin_is_empty_so_a_prompt_cannot_wait() {
    let tree = tree();
    let result = run(&tree, "sh -c 'read line || exit 7'");
    assert_eq!(
        (result.status, result.exit_code),
        (RunStatus::Failed, Some(7))
    );
}

#[test]
fn the_json_form_has_the_reference_keys_in_order() {
    let tree = tree();
    let result = RunResult {
        duration: 0.5,
        ..run(&tree, "sh -c 'echo out'")
    };
    let shown = serde_json::to_string(&result).expect("the result serializes");
    let expected = r#"{"status":"passed","exit_code":0,"duration":0.5,"stdout_tail":"out\n","stderr_tail":""}"#;
    assert_eq!(shown, expected);
    assert_eq!(RunStatus::Timeout.to_string(), "timeout");
}

fn trimmed(written: &[&[u8]], ceilings: &[(u64, u64)]) -> (Vec<u64>, u64) {
    let tree = tree();
    let path = tree.join("capture.log");
    let mut file = File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .expect("the file opens");
    let mut holes = Vec::new();
    for (bytes, (ceiling, hole)) in written.iter().zip(ceilings) {
        std::io::Write::write_all(&mut file, bytes).expect("the bytes are written");
        holes.push(trim(&file, *ceiling, *hole).expect("the trim succeeds"));
    }
    (holes, file.metadata().expect("the file stats").len())
}

#[test]
fn a_file_over_the_ceiling_is_emptied_and_reports_its_hole() {
    assert_eq!(trimmed(&[&[b'x'; 300]], &[(100, 0)]), (vec![300], 0));
}

#[test]
fn a_short_tail_written_past_the_hole_survives_the_next_poll() {
    assert_eq!(
        trimmed(&[&[b'x'; 300], b"tail"], &[(100, 0), (100, 300)]),
        (vec![300, 300], 304)
    );
}

#[test]
fn live_bytes_past_the_ceiling_trim_again_from_the_new_hole() {
    assert_eq!(
        trimmed(&[&[b'x'; 300], &[b'y'; 200]], &[(100, 0), (100, 300)]),
        (vec![300, 500], 0)
    );
}

#[derive(Deserialize)]
struct TailCase {
    bytes: Vec<u8>,
    limit: usize,
    tail: String,
}

#[test]
fn tail_cases() {
    let cases: Vec<TailCase> = fixture("testrun", "tail");
    let tree = tree();
    for (index, case) in cases.iter().enumerate() {
        let path = tree
            .write_file(format!("capture-{index}"), &case.bytes)
            .expect("the bytes are written");
        let mut file = File::open(&path).expect("the file opens");
        let shown = tail(&mut file, case.limit).expect("the tail reads");
        assert_eq!(shown, case.tail, "{:?} limit {}", case.bytes, case.limit);
    }
}

#[derive(Deserialize)]
struct NotStartedCase {
    name: String,
    layout: Vec<serde_json::Value>,
    cwd: String,
    command: String,
    result: (String, Option<i32>, String, String),
}

#[test]
fn not_started_cases() {
    let cases: Vec<NotStartedCase> = fixture("testrun", "not_started");
    for case in cases {
        let tree = crate::fixtures::build(&case.layout);
        let line = TestCommand {
            cwd: &tree.join(&case.cwd),
            ..command(&tree, &case.command, LONG)
        };
        let result = ran(&line, &env_for(&tree), BOUNDS);
        let found = (
            result.status.to_string(),
            result.exit_code,
            result.stdout_tail,
            result.stderr_tail,
        );
        assert_eq!(found, case.result, "{}", case.name);
    }
}
