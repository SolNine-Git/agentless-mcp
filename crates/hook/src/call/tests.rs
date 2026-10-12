use std::ffi::OsString;
use std::time::Duration;

use super::{Call, Phase};
use crate::Output;
use crate::record::Outcome;
use crate::testing::{Sandbox, Steps};
use crate::verdict::Decision;

fn call(offsets: &[Duration], log: Option<OsString>) -> Call {
    Call::new(Box::new(Steps::new(offsets)), log)
}

fn record_of(call: &Call) -> crate::record::Record {
    match &*call.lock() {
        Phase::Running(record) => (**record).clone(),
        Phase::Settled => panic!("the call is settled"),
    }
}

#[test]
fn the_budget_is_what_is_left_before_the_deadline_less_the_log_reserve() {
    let fresh = call(&[Duration::ZERO], None);
    assert_eq!(
        fresh.budget(Some(Duration::from_secs(2))),
        Some(Duration::from_secs(2))
    );
    assert_eq!(fresh.budget(None), Some(Duration::from_millis(9500)));
    let late = call(&[Duration::ZERO, Duration::from_millis(9400)], None);
    assert_eq!(late.budget(None), Some(Duration::from_millis(100)));
    let spent = call(&[Duration::ZERO, Duration::from_millis(9500)], None);
    assert_eq!(spent.budget(Some(Duration::from_secs(2))), None);
}

#[test]
fn the_blank_record_has_the_fixed_time_and_no_outcome() {
    let fresh = call(&[Duration::ZERO], None);
    let record = record_of(&fresh);
    assert!((record.ts - 1_760_000_000.0).abs() < f64::EPSILON);
    assert_eq!(record.outcome, None);
    assert_eq!(record.latency_ms, None);
}

#[test]
fn a_sigterm_after_the_decision_changes_nothing() {
    let settled = call(&[Duration::ZERO], None);
    let decision = Decision {
        code: 2,
        stderr: "blocked\n".to_owned(),
        ..Decision::default()
    };
    let output = settled.settle(decision);
    assert_eq!(output.code, 2);
    assert_eq!(settled.terminate(&mut settled.lock()), None);
}

#[test]
fn a_sigterm_before_the_decision_logs_terminated_and_the_decision_writes_nothing() {
    let sandbox = Sandbox::new();
    let log = sandbox.path().join("log/erosion.jsonl");
    let running = call(&[Duration::ZERO], Some(log.clone().into_os_string()));
    running.note(|record| record.tool_use_id = Some("t1".to_owned()));
    let terminated = running.terminate(&mut running.lock());
    assert_eq!(terminated, Some(Output::default()));
    let line = std::fs::read_to_string(&log).expect("the log has the record");
    assert!(
        line.contains(r#""outcome": "degraded", "reason": "terminated""#),
        "{line}"
    );
    assert!(line.contains(r#""tool_use_id": "t1""#), "{line}");
    let after = running.settle(Decision {
        code: 2,
        ..Decision::default()
    });
    assert_eq!(after, Output::default());
}

#[test]
fn a_log_that_cannot_be_written_leaves_the_reply_and_names_the_target() {
    let sandbox = Sandbox::new();
    sandbox.put("blocker", "a file, not a directory");
    let target = sandbox.path().join("blocker/erosion.jsonl");
    let blocked = call(&[Duration::ZERO], Some(target.clone().into_os_string()));
    let output = blocked.settle(Decision {
        stdout: "reply\n".to_owned(),
        ..Decision::default()
    });
    assert_eq!(output.stdout, "reply\n");
    let first = output.stderr.lines().next().unwrap_or("(none)").to_owned();
    let expected = format!(
        "erosion gate cannot append to {}: tool_use_id=None",
        target.display()
    );
    assert_eq!(first, expected);
}

#[test]
fn an_unset_or_empty_log_writes_nothing() {
    for log in [None, Some(OsString::new())] {
        let quiet = call(&[Duration::ZERO], log);
        assert_eq!(quiet.settle(Decision::default()), Output::default());
    }
}

#[test]
fn a_note_writes_the_running_record() {
    let running = call(&[Duration::ZERO], None);
    running.note(|record| record.outcome = Some(Outcome::Skipped));
    assert_eq!(record_of(&running).outcome, Some(Outcome::Skipped));
}
