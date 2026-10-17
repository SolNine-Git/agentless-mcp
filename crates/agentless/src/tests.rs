use std::panic;
use std::process;
use std::thread;
use std::time::Duration;

use io::testing::is_child;

use super::{answered, guarded, with_streams};

const DEADLINE: Duration = Duration::from_secs(5);

struct Ran {
    code: Option<i32>,
    err: String,
}

// Runs the test `name` of this binary as a child process under the deadline.
fn child(name: &str) -> Ran {
    let (status, err) = io::testing::run_child(name, DEADLINE);
    Ran {
        code: status.code(),
        err,
    }
}

#[test]
fn a_panic_on_a_helper_thread_prints_one_line_and_exits_1_within_5_s() {
    let ran = child("tests::child_a_helper_thread_panics");

    assert_eq!(ran.code, Some(1), "{}", ran.err);
    assert!(
        ran.err
            .starts_with("agentless-mcp: internal error: the helper broke ("),
        "{}",
        ran.err
    );
    assert_eq!(ran.err.matches('\n').count(), 1, "{}", ran.err);
}

// The child of the test above: a scoped helper panics while the caller holds both streams.
#[test]
fn child_a_helper_thread_panics() {
    if !is_child("tests::child_a_helper_thread_panics") {
        return;
    }
    let caller = thread::current().id();
    let work = || -> u8 {
        assert_ne!(thread::current().id(), caller, "the work runs on a helper");
        panic!("the helper broke")
    };
    let code = with_streams(|streams| {
        guarded(streams, |_| {
            thread::scope(|scope| io::spawn_or_run(scope, &work).joined())
        })
    });
    process::exit(i32::from(code));
}

#[test]
fn a_gate_that_catches_its_panics_writes_only_its_own_stderr() {
    let ran = child("tests::child_a_gate_catches_two_panics");

    assert_eq!(
        (ran.code, ran.err.as_str()),
        (Some(0), "the gate caught 2 panics\n")
    );
}

// The child of the test above: the gate catches a panic on its own thread and one on a helper.
#[test]
fn child_a_gate_catches_two_panics() {
    if !is_child("tests::child_a_gate_catches_two_panics") {
        return;
    }
    let code = answered(|| {
        let on_caller = panic::catch_unwind(|| panic!("the gate broke"));
        let on_helper = thread::spawn(|| panic!("the lint broke")).join();
        let caught = usize::from(on_caller.is_err()) + usize::from(on_helper.is_err());
        hook::Output {
            stderr: format!("the gate caught {caught} panics\n"),
            ..hook::Output::default()
        }
    });
    process::exit(i32::from(code));
}
