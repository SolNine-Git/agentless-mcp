use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use app::hooks::Hook;
use kernel::Clock;
use serde_json::json;

use super::{mapped, recorded};
use crate::ledger;
use crate::prompt_map::{MapFn, in_process};
use crate::run::Run;
use crate::testing::{Rig, Steps, WALL_SECONDS, context_of};

const EVENT: &str = "SubagentStart";
const BRIEF: &str = "Find why InvoiceTotal rounds twice; start from `src/billing/report.py`.";

// A clock whose wall time is `seconds` after the test clock's.
struct Later(u64);

impl Clock for Later {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wall(&self) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(WALL_SECONDS + self.0)
    }
}

fn agent_call(session: &str, kind: Option<&str>, prompt: &str, cwd: &PathBuf) -> String {
    let mut input = json!({"description": "Find the cause", "prompt": prompt});
    if let Some(kind) = kind {
        input["subagent_type"] = json!(kind);
    }
    json!({
        "session_id": session,
        "cwd": cwd,
        "hook_event_name": "PreToolUse",
        "tool_name": "Agent",
        "tool_input": input,
    })
    .to_string()
}

fn start(session: &str, agent: &str, kind: &str, cwd: &PathBuf) -> String {
    json!({
        "session_id": session,
        "cwd": cwd,
        "hook_event_name": EVENT,
        "agent_id": agent,
        "agent_type": kind,
    })
    .to_string()
}

impl Rig {
    fn brief(&self, stdin: &str) {
        self.brief_at(&Steps::fixed(), stdin);
    }

    fn brief_at(&self, clock: &dyn Clock, stdin: &str) {
        let run = self.run(Hook::BriefRecord, clock);
        let stdout = Rig::answer(&run, |facts| {
            recorded(&run, &self.services, &mut stdin.as_bytes(), facts)
        });
        assert_eq!(stdout, "");
    }

    fn map_with(&self, clock: &dyn Clock, map: MapFn, deadline: Duration, stdin: &str) -> String {
        let run = Run {
            deadline,
            ..self.run(Hook::SubagentMap, clock)
        };
        Rig::answer(&run, |facts| {
            mapped(&run, map, &self.services, &mut stdin.as_bytes(), facts)
        })
    }

    fn map(&self, stdin: &str) -> String {
        self.map_with(&Steps::fixed(), in_process, Duration::from_secs(30), stdin)
    }

    fn records(&self, session: &str) -> usize {
        let dir = ledger::briefs_dir(&self.ledger(), session);
        std::fs::read_dir(dir).map_or(0, Iterator::count)
    }
}

#[test]
fn a_subagent_gets_the_map_files_of_its_brief_that_the_brief_does_not_name() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    rig.brief(&agent_call("s1", Some("Explore"), BRIEF, &root));
    assert_eq!(rig.records("s1"), 1);

    let context = context_of(&rig.map(&start("s1", "a1", "Explore", &root)), EVENT);

    let expected = "Repository map (agentless-mcp): for src/billing/report.py, InvoiceTotal in this \
                    brief, the files that rank first and that the brief does not name are \
                    src/billing/invoice.py and config/settings.toml.";
    assert_eq!(context, expected);
    assert_eq!(rig.records("s1"), 0);
    assert_eq!(rig.map(&start("s1", "a2", "Explore", &root)), "");
    assert_eq!(rig.reasons(), ["recorded", "emitted", "no_record"]);
}

#[test]
fn two_pending_briefs_of_one_type_are_cleared_and_pair_with_nothing() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    rig.brief(&agent_call("s1", Some("Explore"), BRIEF, &root));
    rig.brief(&agent_call(
        "s1",
        Some("Explore"),
        "Count the callers.",
        &root,
    ));
    rig.brief(&agent_call("s1", Some("Plan"), BRIEF, &root));
    assert_eq!(rig.records("s1"), 3);

    assert_eq!(rig.map(&start("s1", "a1", "Explore", &root)), "");
    assert_eq!(rig.map(&start("s1", "a2", "Explore", &root)), "");
    assert!(!rig.map(&start("s1", "a3", "Plan", &root)).is_empty());

    assert_eq!(rig.records("s1"), 0);
    let expected = [
        "recorded",
        "recorded",
        "recorded",
        "ambiguous_pairing",
        "no_record",
        "emitted",
    ];
    assert_eq!(rig.reasons(), expected);
}

#[test]
fn a_brief_pairs_only_within_its_session_type_and_minute() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    rig.brief(&agent_call("s1", Some("Explore"), BRIEF, &root));
    assert_eq!(rig.map(&start("s2", "a1", "Explore", &root)), "");
    assert_eq!(rig.map(&start("s1", "a1", "Plan", &root)), "");
    let late = Later(PENDING_SECONDS);
    let stdout = rig.map_with(
        &late,
        in_process,
        Duration::from_secs(30),
        &start("s1", "a1", "Explore", &root),
    );
    assert_eq!(stdout, "");
    assert_eq!(rig.records("s1"), 1);
    let just_in_time = Later(PENDING_SECONDS - 1);
    let stdout = rig.map_with(
        &just_in_time,
        in_process,
        Duration::from_secs(30),
        &start("s1", "a1", "Explore", &root),
    );
    assert!(!stdout.is_empty());
    let expected = ["recorded", "no_record", "no_record", "no_record", "emitted"];
    assert_eq!(rig.reasons(), expected);
}

const PENDING_SECONDS: u64 = 60;

#[test]
fn a_fork_is_not_recorded_and_a_call_with_no_type_is_general_purpose() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    rig.brief(&agent_call("s1", Some("fork"), BRIEF, &root));
    assert_eq!(rig.records("s1"), 0);
    rig.brief(&agent_call("s1", None, BRIEF, &root));
    assert!(
        !rig.map(&start("s1", "a1", "general-purpose", &root))
            .is_empty()
    );
    assert_eq!(rig.reasons(), ["fork", "recorded", "emitted"]);
}

#[test]
fn a_brief_with_no_seed_is_recorded_and_maps_nothing() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    rig.brief(&agent_call("s1", Some("Explore"), "Look around.", &root));
    assert_eq!(rig.map(&start("s1", "a1", "Explore", &root)), "");
    let named = "Read `src/billing/invoice.py`, `src/billing/report.py` and \
                 `config/settings.toml` for InvoiceTotal.";
    rig.brief(&agent_call("s1", Some("Explore"), named, &root));
    assert_eq!(rig.map(&start("s1", "a2", "Explore", &root)), "");
    rig.brief(&agent_call(
        "s1",
        Some("Explore"),
        "Why does NothingMatches fail?",
        &root,
    ));
    assert_eq!(rig.map(&start("s1", "a3", "Explore", &root)), "");
    let expected = [
        "recorded",
        "no_seed",
        "recorded",
        "no_new_file",
        "recorded",
        "no_seed_resolved",
    ];
    assert_eq!(rig.reasons(), expected);
}

#[test]
fn a_malformed_payload_records_and_maps_nothing() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    let no_prompt = json!({"session_id": "s1", "tool_input": {"subagent_type": "Explore"}});
    for stdin in [
        "{not json".to_owned(),
        json!({"tool_input": {}}).to_string(),
        no_prompt.to_string(),
    ] {
        rig.brief(&stdin);
    }
    let no_type = json!({"session_id": "s1", "agent_id": "a1", "cwd": root});
    for stdin in ["[1]".to_owned(), no_type.to_string()] {
        assert_eq!(rig.map(&stdin), "");
    }
    assert_eq!(rig.records("s1"), 0);
    let expected = [
        "malformed_payload",
        "no_session_id",
        "no_prompt",
        "malformed_payload",
        "no_agent_type",
    ];
    assert_eq!(rig.reasons(), expected);
}

const STUCK: MapFn = |_, _, _| {
    loop {
        thread::park();
    }
};

const FAILING: MapFn =
    |_, _, _| Err(kernel::Error::OperationFailed("no cache root".to_owned()).into());

#[test]
fn a_late_or_failed_map_is_silent_and_uses_the_record() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    let clock = Steps::fixed();
    for (agent, map, deadline) in [
        ("a1", STUCK, Duration::from_millis(100)),
        ("a2", FAILING, Duration::from_secs(5)),
    ] {
        rig.brief(&agent_call("s1", Some("Explore"), BRIEF, &root));
        let stdout = rig.map_with(&clock, map, deadline, &start("s1", agent, "Explore", &root));
        assert_eq!(stdout, "");
    }
    assert_eq!(rig.records("s1"), 0);
    let expected = ["recorded", "deadline", "recorded", "map_failed"];
    assert_eq!(rig.reasons(), expected);
}

// A record past the pairing window pairs with no subagent, so each new record removes them.
#[test]
fn a_new_brief_removes_each_record_past_the_pairing_window() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    rig.brief(&agent_call("s1", Some("Explore"), BRIEF, &root));
    rig.brief(&agent_call("s1", Some("Plan"), BRIEF, &root));
    rig.brief(&agent_call("s2", Some("Plan"), BRIEF, &root));
    assert_eq!((rig.records("s1"), rig.records("s2")), (2, 1));

    rig.brief_at(&Later(60), &agent_call("s1", Some("Plan"), BRIEF, &root));
    assert_eq!((rig.records("s1"), rig.records("s2")), (1, 1));
    rig.brief_at(&Later(90), &agent_call("s1", Some("Explore"), BRIEF, &root));
    assert_eq!(rig.records("s1"), 2);
    assert!(!rig.map(&start("s1", "a1", "Plan", &root)).is_empty());
}
