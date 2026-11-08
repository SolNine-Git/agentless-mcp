use std::collections::BTreeSet;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{CARD_CAP, CardFn, ReadCard, answer, cut, in_process, read_card};
use crate::ledger::CAP;
use crate::testing::{Rig, Steps, context_of};
use crate::text::sha256_hex;

const EVENT: &str = "PostToolUse";
const HEAD: &str = "File card (agentless-mcp), first read of src/billing/invoice.py in this \
                    context:\nsrc/billing/invoice.py  python  7 lines  ids: \
                    py:src/billing/invoice.py::<QualifiedName>\n";

fn read_payload(session: &str, agent: Option<&str>, file: &Path) -> String {
    let mut payload = json!({
        "session_id": session,
        "transcript_path": "/nonexistent",
        "cwd": "/nonexistent",
        "hook_event_name": EVENT,
        "tool_name": "Read",
        "tool_input": {"file_path": file},
        "tool_response": {"type": "text"},
    });
    if let Some(agent) = agent {
        payload["agent_id"] = json!(agent);
        payload["agent_type"] = json!("Explore");
    }
    payload.to_string()
}

fn bash_payload(session: &str, command: &str, cwd: &Path) -> String {
    json!({
        "session_id": session,
        "cwd": cwd,
        "hook_event_name": EVENT,
        "tool_name": "Bash",
        "tool_input": {"command": command, "description": "Read the file"},
    })
    .to_string()
}

impl Rig {
    fn hook<'a>(&'a self, clock: &'a Steps, card: CardFn, deadline: Duration) -> ReadCard<'a> {
        ReadCard {
            run: self.run(clock),
            card,
            deadline,
        }
    }

    fn read_with(&self, hook: &ReadCard<'_>, stdin: &str) -> String {
        let mut stdout = Vec::new();
        answer(hook, &self.services, &mut stdin.as_bytes(), &mut stdout);
        String::from_utf8(stdout).expect("UTF-8 stdout")
    }

    fn read(&self, stdin: &str) -> String {
        let clock = Steps::fixed();
        self.read_with(
            &self.hook(&clock, in_process, Duration::from_secs(30)),
            stdin,
        )
    }
}

#[test]
fn a_first_read_of_a_source_file_gets_its_file_card_and_exits_0() {
    let rig = Rig::new();
    let tmp = rig.sandbox.path().as_os_str().to_owned();
    let env = rig
        .sandbox
        .env(&[("TMPDIR", tmp), ("AGENTLESS_GATE_LOG", rig.log.clone())]);
    let stdin = read_payload("s1", None, &rig.file(Rig::INVOICE));
    let mut stdout = Vec::new();

    let code = read_card(&env, &rig.services, &mut stdin.as_bytes(), &mut stdout);

    assert_eq!(code, 0);
    let stdout = String::from_utf8(stdout).expect("UTF-8 stdout");
    let context = context_of(&stdout, EVENT);
    assert!(context.starts_with(HEAD), "{context}");
    assert!(!context.ends_with('\n'), "{context:?}");
    let entries = rig.sandbox.path().join("agentless_ledger");
    let main = entries.join(sha256_hex(b"s1")).join("main");
    assert_eq!(std::fs::read_dir(&main).map(Iterator::count).ok(), Some(1));
    assert_eq!(rig.reasons(), ["emitted"]);
}

#[test]
fn a_context_gets_one_card_per_file_and_a_subagent_its_own() {
    let rig = Rig::new();
    let invoice = rig.file(Rig::INVOICE);
    assert!(!rig.read(&read_payload("s1", None, &invoice)).is_empty());
    assert_eq!(rig.read(&read_payload("s1", None, &invoice)), "");
    assert!(
        !rig.read(&read_payload("s1", Some("a1"), &invoice))
            .is_empty()
    );
    assert_eq!(rig.read(&read_payload("s1", Some("a1"), &invoice)), "");
    assert!(
        !rig.read(&read_payload("s1", Some("a2"), &invoice))
            .is_empty()
    );
    assert!(!rig.read(&read_payload("s2", None, &invoice)).is_empty());
    let expected = ["emitted", "held", "emitted", "held", "emitted", "emitted"];
    assert_eq!(rig.reasons(), expected);
}

#[test]
fn an_edit_makes_the_card_stale_so_the_next_read_gets_a_new_one() {
    let rig = Rig::new();
    let invoice = rig.file(Rig::INVOICE);
    assert!(!rig.read(&read_payload("s1", None, &invoice)).is_empty());
    let edited = std::fs::read_to_string(&invoice).expect("the file") + "\n\nRATE = 2\n";
    drop(rig.sandbox.put(&format!("repo/{}", Rig::INVOICE), edited));
    let context = context_of(&rig.read(&read_payload("s1", None, &invoice)), EVENT);
    assert!(context.contains("  python  10 lines  "), "{context}");
    assert_eq!(rig.reasons(), ["emitted", "emitted"]);
}

#[test]
fn a_bash_view_of_one_source_file_gets_the_card_of_that_file() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    let billing = root.join("src/billing");
    for (session, command, cwd) in [
        ("s1", "cat src/billing/invoice.py", &root),
        ("s2", "sed -n '1,5p' invoice.py", &billing),
        ("s3", "grep -n 'round|cents' src/billing/invoice.py", &root),
    ] {
        let context = context_of(&rig.read(&bash_payload(session, command, cwd)), EVENT);
        assert!(context.starts_with(HEAD), "{command}: {context}");
    }
    for (session, command) in [
        ("s4", "cat src/billing/invoice.py | head"),
        ("s5", "cat src/billing/invoice.py src/billing/report.py"),
        ("s6", "ls src"),
    ] {
        assert_eq!(
            rig.read(&bash_payload(session, command, &root)),
            "",
            "{command}"
        );
    }
    let expected = [
        "emitted",
        "emitted",
        "emitted",
        "not_a_read",
        "not_a_read",
        "not_a_read",
    ];
    assert_eq!(rig.reasons(), expected);
}

#[test]
fn a_read_that_is_not_of_a_source_file_in_a_repository_is_silent() {
    let rig = Rig::new();
    drop(rig.sandbox.put("elsewhere/loose.py", "LOOSE = 1\n"));
    let loose = rig.sandbox.path().join("elsewhere/loose.py");
    let missing = rig.file("src/billing/missing.py");
    let folder = rig.file("src/billing");
    let cases = [
        (rig.file(Rig::SETTINGS), "not_source"),
        (loose, "outside_git"),
        (missing, "no_file"),
        (folder, "no_file"),
    ];
    for (session, (file, _)) in cases.iter().enumerate() {
        let stdin = read_payload(&format!("s{session}"), None, file);
        assert_eq!(rig.read(&stdin), "", "{}", file.display());
    }
    let expected: Vec<&str> = cases.iter().map(|(_, reason)| *reason).collect();
    assert_eq!(rig.reasons(), expected);
}

#[test]
fn a_payload_that_is_not_a_read_is_silent() {
    let rig = Rig::new();
    let invoice = rig.file(Rig::INVOICE);
    let grep = json!({"session_id": "s1", "tool_name": "Grep", "tool_input": {"pattern": "x"}});
    let numeric_agent = json!({
        "session_id": "s1",
        "agent_id": 7,
        "tool_name": "Read",
        "tool_input": {"file_path": invoice},
    });
    let no_path = json!({"session_id": "s1", "tool_name": "Read", "tool_input": {}});
    for stdin in [
        String::new(),
        "{not json".to_owned(),
        "[1, 2]".to_owned(),
        json!({"tool_name": "Read"}).to_string(),
        grep.to_string(),
        numeric_agent.to_string(),
        no_path.to_string(),
    ] {
        assert_eq!(rig.read(&stdin), "", "{stdin}");
    }
    let expected = [
        "no_session_id",
        "malformed_payload",
        "malformed_payload",
        "no_session_id",
        "not_a_read",
        "malformed_payload",
        "not_a_read",
    ];
    assert_eq!(rig.reasons(), expected);
}

// A card that never returns: its thread parks until the test process exits.
const STUCK: CardFn = |_, _, _| {
    loop {
        thread::park();
    }
};

const PANICKING: CardFn = |_, _, _| panic!("a defect in the card");

const FAILING: CardFn =
    |_, _, _| Err(kernel::Error::OperationFailed("no cache root".to_owned()).into());

const NO_CARD: CardFn = |_, _, _| Ok(None);

#[test]
fn a_card_past_the_deadline_is_silent_and_uses_the_one_try() {
    let rig = Rig::new();
    let clock = Steps::fixed();
    let deadline = Duration::from_millis(100);
    let stdin = read_payload("s1", None, &rig.file(Rig::INVOICE));
    let started = Instant::now();
    assert_eq!(
        rig.read_with(&rig.hook(&clock, STUCK, deadline), &stdin),
        ""
    );
    assert!(started.elapsed() < deadline + Duration::from_secs(1));
    assert_eq!(rig.read(&stdin), "");
    assert_eq!(rig.reasons(), ["deadline", "held"]);
}

#[test]
fn a_panicking_failing_or_missing_card_is_silent() {
    let rig = Rig::new();
    let clock = Steps::fixed();
    let invoice = rig.file(Rig::INVOICE);
    for (session, card) in [("s1", PANICKING), ("s2", FAILING), ("s3", NO_CARD)] {
        let hook = rig.hook(&clock, card, Duration::from_secs(5));
        let stdin = read_payload(session, None, &invoice);
        assert_eq!(rig.read_with(&hook, &stdin), "");
    }
    assert_eq!(rig.reasons(), ["internal", "render_failed", "no_file_card"]);
}

#[test]
fn an_untrusted_or_full_ledger_never_emits() {
    let rig = Rig::new();
    let invoice = rig.file(Rig::INVOICE);
    rig.sandbox.mkdir("ledger");
    rig.sandbox.set_mode("ledger", 0o770);
    assert_eq!(rig.read(&read_payload("s1", None, &invoice)), "");
    rig.sandbox.set_mode("ledger", 0o700);
    let main = format!("ledger/{}/main", sha256_hex(b"s2"));
    let dir = rig.sandbox.mkdir(&main);
    for entry in 0..CAP {
        io::write_file(&dir.join(format!("{entry:04}")), b"").expect("an entry");
    }
    assert_eq!(rig.read(&read_payload("s2", None, &invoice)), "");
    assert_eq!(rig.reasons(), ["ledger_storage_unavailable", "ledger_full"]);
}

#[test]
fn the_entry_is_named_by_session_context_path_and_content() {
    let rig = Rig::new();
    let invoice = rig.file(Rig::INVOICE);
    drop(rig.read(&read_payload("s1", Some("a1"), &invoice)));
    let canonical = io::resolve(&invoice).expect("a canonical path");
    let content = std::fs::read(&invoice).expect("the file");
    let mut key = canonical.as_os_str().as_encoded_bytes().to_vec();
    key.push(0);
    key.extend_from_slice(sha256_hex(&content).as_bytes());
    let entry = rig
        .ledger()
        .join(sha256_hex(b"s1"))
        .join(sha256_hex(b"a1"))
        .join(sha256_hex(&key));
    let metadata = std::fs::metadata(&entry).expect("the entry");
    assert_eq!((metadata.is_file(), metadata.len()), (true, 0));
}

#[test]
fn the_log_line_holds_its_keys_in_order() {
    let rig = Rig::new();
    let clock = Steps::new(&[Duration::ZERO, Duration::from_millis(40)]);
    let hook = rig.hook(&clock, in_process, Duration::from_secs(30));
    drop(rig.read_with(
        &hook,
        &read_payload("s1", Some("a1"), &rig.file(Rig::INVOICE)),
    ));
    let lines = rig.lines();
    let keys: BTreeSet<&str> = (lines[0].as_object().expect("an object").keys())
        .map(String::as_str)
        .collect();
    let order = [
        "ts",
        "hook",
        "event",
        "reason",
        "session_id",
        "agent_id",
        "ms",
    ];
    assert_eq!(keys, BTreeSet::from(order));
    let text = std::fs::read_to_string(rig.sandbox.path().join("gate.jsonl")).expect("the log");
    let expected = "{\"ts\": 1760000000.0, \"hook\": \"read-card\", \"event\": \"emitted\", \
                    \"reason\": \"emitted\", \"session_id\": \"s1\", \"agent_id\": \"a1\", \
                    \"ms\": 40}\n";
    assert_eq!(text, expected);
}

#[test]
fn a_card_over_the_cap_keeps_whole_lines_and_a_long_first_line_is_cut() {
    assert_eq!(cut("a\nb\n"), "a\nb");
    let line = "x".repeat(99);
    let card = [line.as_str(); 12].join("\n") + "\n";
    let kept = cut(&card);
    assert_eq!(kept.lines().count(), 10, "{kept}");
    assert!(kept.chars().count() <= CARD_CAP);
    let long = "y".repeat(CARD_CAP + 5);
    assert_eq!(cut(&format!("{long}\nrest")), "y".repeat(CARD_CAP));
}

#[test]
fn the_ledger_claim_holds_under_parallel_reads() {
    let rig = Rig::new();
    let stdin = read_payload("s1", None, &rig.file(Rig::INVOICE));
    let outputs: Vec<String> = thread::scope(|scope| {
        let runs: Vec<_> = (0..4).map(|_| scope.spawn(|| rig.read(&stdin))).collect();
        runs.into_iter()
            .map(|run| run.join().expect("a run"))
            .collect()
    });
    let emitted = outputs.iter().filter(|stdout| !stdout.is_empty()).count();
    assert_eq!(emitted, 1, "{outputs:?}");
    let mut reasons = rig.reasons();
    reasons.sort();
    assert_eq!(reasons, ["emitted", "held", "held", "held"]);
    let lines: Vec<Value> = rig.lines();
    assert!(lines.iter().all(|line| line["hook"] == "read-card"));
}
