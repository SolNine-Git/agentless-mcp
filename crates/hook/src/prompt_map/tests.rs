use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use app::Services;
use app::hooks::Hook;
use serde_json::{Value, json};

use super::seeds::{MAX_CONTEXT_BYTES, context_text, seeds};
use super::{MapFn, PromptMap, answer, in_process, prompt_map};
use crate::run::Run;
use crate::testing::{Sandbox, Steps};
use crate::text::sha256_hex;

const PREFIX: &str = "Repository map (agentless-mcp): for ";
const INVOICE: &str = "src/billing/invoice.py";
const MARKERS: &str = "markers";
const LOG: &str = "gate.jsonl";
// The template of the bench's native arm with no issue, as the measured runs sent it.
const BENCH_TEMPLATE: &str = "You are a code exploration specialist. Explore this repository to \
find the\nsource files and line ranges most relevant to understanding and fixing the\nfollowing \
issue. Do NOT make any code changes.\n\nUse Glob, Grep, and Read tools to explore the codebase. \
Focus on finding\nthe ROOT CAUSE, not just symptom locations.\n\nAfter exploration, output your \
findings in EXACTLY this format:\n\nRELEVANT_FILES:\n- path/to/file1.py:10-50\n- \
path/to/file2.py:1-100\n\nFocus on the root cause. Limit to top 20 most relevant regions.\n\n\
ISSUE:\n\n";

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|&item| item.to_owned()).collect()
}

// The seeds of `prompt` when the files in `files` exist, and every path that the predicate saw.
fn seeded(prompt: &str, files: &[&str]) -> (Vec<String>, Vec<String>) {
    let asked = RefCell::new(Vec::new());
    let found = seeds(prompt, &|rel| {
        asked.borrow_mut().push(rel.to_owned());
        files.contains(&rel)
    });
    (found, asked.into_inner())
}

// The expected values come from the measured prototype's own function on the same text.
#[test]
fn seeds_take_files_then_backticked_names_then_camel_case_then_snake_case() {
    let prompt = "See src/app.py and `lib/util.py`. `parse_config()` fails in \
                  `ConfigLoader.load`; HttpClient retries; max_retries is wrong. `ab` `x()`";
    let (found, _) = seeded(prompt, &["src/app.py", "lib/util.py"]);
    let expected = [
        "src/app.py",
        "lib/util.py",
        "parse_config",
        "ConfigLoader.load",
        "ConfigLoader",
        "HttpClient",
        "max_retries",
    ];
    assert_eq!(found, strings(&expected));
}

#[test]
fn seeds_stop_at_twelve() {
    let prompt: Vec<String> = (1..=15).map(|n| format!("name_{n:02}")).collect();
    let (found, _) = seeded(&prompt.join(" "), &[]);
    assert_eq!(found, prompt[..12]);
}

// A leading slash is read below the root, as the prototype reads it; nothing else leaves it.
#[test]
fn no_seed_names_a_path_outside_the_root() {
    let prompt = "../etc/passwd.txt a//b.py ./x.py /abs/file.py `../up.py` `/abs/other.py`";
    let files = [
        "abs/file.py",
        "abs/other.py",
        "etc/passwd.txt",
        "x.py",
        "b.py",
        "up.py",
    ];
    let (found, asked) = seeded(prompt, &files);
    assert_eq!(found, strings(&["abs/file.py", "abs/other.py"]));
    let asked: BTreeSet<String> = asked.into_iter().collect();
    let below: BTreeSet<String> = strings(&["abs/file.py", "abs/other.py"])
        .into_iter()
        .collect();
    assert_eq!(asked, below);
}

#[test]
fn a_path_token_is_the_greedy_match_of_the_prototype() {
    let prompt = "archive.tar.gz file.toolongext end.py. .hidden.cfg -x.py a.b";
    let files = [
        "archive.tar.gz",
        "file.toolon",
        "end.py",
        ".hidden.cfg",
        "-x.py",
    ];
    let (found, asked) = seeded(prompt, &files);
    assert_eq!(found, strings(&files));
    assert_eq!(asked.last().map(String::as_str), Some("a.b"));
}

#[test]
fn a_word_seed_needs_word_boundaries_and_four_characters() {
    let prompt = "naïve_case FooBar_baz FooBaré x FooBAR y ABCDef Foo2Bar _lead_under trail_under_ \
                  dou__ble ok_2x Ab9";
    let (found, _) = seeded(prompt, &[]);
    assert_eq!(found, strings(&["FooBAR", "Foo2Bar", "ok_2x"]));
}

#[test]
fn a_backticked_name_is_trimmed_and_holds_one_to_120_characters_on_one_line() {
    let long = format!("`{}`", "y".repeat(121));
    assert_eq!(seeded(&long, &[]).0, Vec::<String>::new());
    let capped = "y".repeat(120);
    assert_eq!(seeded(&format!("`{capped}`"), &[]).0, [capped]);
    assert_eq!(seeded("`a\nb` `Real.name`", &[]).0, Vec::<String>::new());
    let trimmed = seeded("`  Spaced.name()  ` and `ab.c` and `x()` and `9lives`", &[]);
    assert_eq!(trimmed.0, strings(&["Spaced.name", "ab.c"]));
}

#[test]
fn the_bench_template_alone_and_prose_give_no_seed() {
    assert_eq!(seeded(BENCH_TEMPLATE, &[]).0, Vec::<String>::new());
    let prose = "The page crashes when I click the button twice.";
    assert_eq!(seeded(prose, &[]).0, Vec::<String>::new());
}

#[test]
fn the_context_names_four_names_and_up_to_two_files() {
    let one = context_text(&strings(&["a_b"]), &strings(&["x.py"]));
    let text = "Repository map (agentless-mcp): for a_b in this task, the files that rank first \
                are x.py.";
    assert_eq!(one.as_deref(), Some(text));
    let names = strings(&["n_1", "n_2", "n_3", "n_4", "n_5"]);
    let two = context_text(&names, &strings(&["x.py", "y.py", "z.py"]));
    let text = "Repository map (agentless-mcp): for n_1, n_2, n_3, n_4 in this task, the files \
                that rank first are x.py and y.py.";
    assert_eq!(two.as_deref(), Some(text));
    assert_eq!(context_text(&[], &strings(&["x.py"])), None);
    assert_eq!(context_text(&names, &[]), None);
}

#[test]
fn the_byte_cap_drops_the_second_file_and_then_the_sentence() {
    let names = strings(&["a_b"]);
    let wide = format!("{}.py", "w".repeat(400));
    let both = context_text(&names, &[wide.clone(), wide.clone()]).expect("one file fits");
    assert!(both.ends_with(&format!("are {wide}.")), "{both}");
    assert!(both.len() <= MAX_CONTEXT_BYTES);
    let wider = format!("{}.py", "w".repeat(800));
    assert_eq!(context_text(&names, &[wider]), None);
}

#[test]
fn the_context_renders_the_fixture_row() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../prompts/fixtures/render.json"
    );
    let fixture: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the render fixture"))
            .expect("JSON");
    let row = &fixture["messages"]["prompt_map_context"];
    let names: Vec<String> = row["arguments"]["seeds"]
        .as_str()
        .expect("the names")
        .split(", ")
        .map(str::to_owned)
        .collect();
    let files: Vec<String> = row["arguments"]["paths"]
        .as_str()
        .expect("the files")
        .split(" and ")
        .map(str::to_owned)
        .collect();
    assert_eq!(
        context_text(&names, &files).as_deref(),
        row["text"].as_str()
    );
}

// A sandbox with a two-file repository, the services and the gate log of its runs.
struct Rig {
    sandbox: Sandbox,
    services: Services,
    log: OsString,
}

impl Rig {
    fn new() -> Self {
        let sandbox = Sandbox::new();
        sandbox.mkdir("repo");
        sandbox.git(&["init", "--quiet", "-b", "main"]);
        let invoice = "class InvoiceTotal:\n    def rounded(self):\n        \
                       return round_cents(self.amount)\n\n\ndef round_cents(value):\n    \
                       return round(value, 2)\n";
        drop(sandbox.put(&format!("repo/{INVOICE}"), invoice));
        let report = "from src.billing.invoice import InvoiceTotal\n\n\n\
                      def monthly_report(items):\n    return [InvoiceTotal() for _ in items]\n";
        drop(sandbox.put("repo/src/billing/report.py", report));
        sandbox.git(&["add", "-A"]);
        sandbox.git(&["commit", "--quiet", "-m", "fixture"]);
        // No repository above the sandbox can answer for a directory outside the fixture.
        let ceiling = sandbox.path().as_os_str().to_owned();
        let services = Services::new(&sandbox.env(&[("GIT_CEILING_DIRECTORIES", ceiling)]));
        let log = sandbox.path().join(LOG).into_os_string();
        Self {
            sandbox,
            services,
            log,
        }
    }

    fn markers(&self) -> PathBuf {
        self.sandbox.path().join(MARKERS)
    }

    fn hook<'a>(&'a self, clock: &'a Steps, map: MapFn, deadline: Duration) -> PromptMap<'a> {
        let run = Run {
            hook: Hook::PromptMap,
            ledger: self.sandbox.path().join("ledger"),
            log: Some(self.log.as_os_str()),
            cwd: None,
            clock,
            deadline,
        };
        PromptMap {
            run,
            markers: self.markers(),
            map,
        }
    }

    fn run_with(&self, hook: &PromptMap<'_>, stdin: &str) -> String {
        let mut stdout = Vec::new();
        answer(hook, &self.services, &mut stdin.as_bytes(), &mut stdout);
        String::from_utf8(stdout).expect("UTF-8 stdout")
    }

    fn run(&self, stdin: &str) -> String {
        let clock = Steps::fixed();
        self.run_with(
            &self.hook(&clock, in_process, Duration::from_secs(30)),
            stdin,
        )
    }

    fn lines(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.sandbox.path().join(LOG)).expect("the gate log");
        text.lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect()
    }

    fn reasons(&self) -> Vec<String> {
        let lines = self.lines();
        let reasons = lines
            .iter()
            .map(|line| line["reason"].as_str().unwrap_or("?"));
        reasons.map(str::to_owned).collect()
    }
}

fn payload(session: &str, prompt: &str, cwd: &Path) -> String {
    json!({
        "session_id": session,
        "transcript_path": "/nonexistent",
        "cwd": cwd.to_str().expect("a UTF-8 path"),
        "permission_mode": "default",
        "hook_event_name": "UserPromptSubmit",
        "prompt": prompt,
    })
    .to_string()
}

const SEEDED: &str = "Totals are off by a cent in `src/billing/invoice.py` (InvoiceTotal).";

fn context_of(stdout: &str) -> String {
    let reply: Value = serde_json::from_str(stdout).expect("one JSON object");
    let output = &reply["hookSpecificOutput"];
    assert_eq!(
        reply.as_object().map(serde_json::Map::len),
        Some(1),
        "{stdout}"
    );
    assert_eq!(output["hookEventName"], "UserPromptSubmit", "{stdout}");
    output["additionalContext"]
        .as_str()
        .expect("a context string")
        .to_owned()
}

#[test]
fn a_first_prompt_that_names_a_file_gets_the_map_files_and_exits_0() {
    let rig = Rig::new();
    let tmp = rig.sandbox.path().as_os_str().to_owned();
    let env = rig
        .sandbox
        .env(&[("TMPDIR", tmp), ("AGENTLESS_GATE_LOG", rig.log.clone())]);
    let stdin = payload("s1", SEEDED, &rig.sandbox.root());
    let mut stdout = Vec::new();

    let code = prompt_map(&env, &rig.services, &mut stdin.as_bytes(), &mut stdout);

    assert_eq!(code, 0);
    let stdout = String::from_utf8(stdout).expect("UTF-8 stdout");
    let context = context_of(&stdout);
    let head = format!(
        "{PREFIX}{INVOICE}, InvoiceTotal in this task, the files that rank first are {INVOICE}"
    );
    assert!(context.starts_with(&head), "{context}");
    assert!(
        stdout.starts_with("{\"hookSpecificOutput\": {\"hookEventName\": "),
        "{stdout}"
    );
    let marker = rig
        .sandbox
        .path()
        .join("agentless_prompt_map")
        .join(sha256_hex(b"s1"));
    assert!(marker.is_file(), "{}", marker.display());
    assert_eq!(rig.reasons(), ["emitted"]);
}

#[test]
fn a_session_gets_one_context_and_another_session_its_own() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    assert!(!rig.run(&payload("s1", SEEDED, &root)).is_empty());
    assert_eq!(rig.run(&payload("s1", SEEDED, &root)), "");
    assert!(!rig.run(&payload("s2", SEEDED, &root)).is_empty());
    assert_eq!(rig.reasons(), ["emitted", "already_enriched", "emitted"]);
}

#[test]
fn a_prompt_without_a_seed_keeps_the_try_and_unresolved_seeds_stay_silent() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    assert_eq!(
        rig.run(&payload("s1", "Totals are off by a cent.", &root)),
        ""
    );
    assert!(!rig.markers().exists());
    assert_eq!(
        rig.run(&payload("s2", "NothingMatchesHere breaks.", &root)),
        ""
    );
    assert!(!rig.run(&payload("s1", SEEDED, &root)).is_empty());
    assert_eq!(rig.reasons(), ["no_seed", "no_seed_resolved", "emitted"]);
}

#[test]
fn a_payload_that_is_not_a_prompt_is_silent() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    let no_prompt = json!({"session_id": "s1", "prompt": 7, "cwd": root}).to_string();
    for stdin in [
        "",
        "{not json",
        "[1, 2]",
        r#"{"prompt": "x"}"#,
        no_prompt.as_str(),
    ] {
        assert_eq!(rig.run(stdin), "", "{stdin}");
    }
    let expected = [
        "no_session_id",
        "malformed_payload",
        "malformed_payload",
        "no_session_id",
        "no_prompt",
    ];
    assert_eq!(rig.reasons(), expected);
}

// A map that never returns: its thread parks until the test process exits.
const STUCK: MapFn = |_, _, _| {
    loop {
        thread::park();
    }
};

const PANICKING: MapFn = |_, _, _| panic!("a defect in the map");

const FAILING: MapFn =
    |_, _, _| Err(kernel::Error::OperationFailed("no cache root".to_owned()).into());

#[test]
fn a_map_past_the_deadline_is_silent_and_not_waited_for() {
    let rig = Rig::new();
    let clock = Steps::fixed();
    let deadline = Duration::from_millis(100);
    let started = Instant::now();
    let stdout = rig.run_with(
        &rig.hook(&clock, STUCK, deadline),
        &payload("s1", SEEDED, &rig.sandbox.root()),
    );
    assert_eq!(stdout, "");
    assert!(started.elapsed() < deadline + Duration::from_secs(1));
    assert_eq!(rig.reasons(), ["deadline"]);
}

#[test]
fn a_panicking_or_failing_map_is_silent() {
    let rig = Rig::new();
    let clock = Steps::fixed();
    let root = rig.sandbox.root();
    for (session, map) in [("s1", PANICKING), ("s2", FAILING)] {
        let hook = rig.hook(&clock, map, Duration::from_secs(5));
        assert_eq!(rig.run_with(&hook, &payload(session, SEEDED, &root)), "");
    }
    assert_eq!(rig.reasons(), ["internal", "map_failed"]);
}

#[test]
fn an_untrusted_marker_directory_or_marker_name_never_emits() {
    let rig = Rig::new();
    let root = rig.sandbox.root();
    rig.sandbox.mkdir(MARKERS);
    rig.sandbox.set_mode(MARKERS, 0o770);
    assert_eq!(rig.run(&payload("s1", SEEDED, &root)), "");
    assert_eq!(rig.run(&payload("s1", SEEDED, &root)), "");
    rig.sandbox.set_mode(MARKERS, 0o700);
    let name = format!("{MARKERS}/{}", sha256_hex(b"s2"));
    rig.sandbox.symlink(&name, "<SANDBOX>/elsewhere");
    assert_eq!(rig.run(&payload("s2", SEEDED, &root)), "");
    let unavailable = "marker_storage_unavailable";
    assert_eq!(rig.reasons(), [unavailable, unavailable, unavailable]);
}

#[test]
fn the_log_line_holds_its_keys_in_order() {
    let rig = Rig::new();
    let clock = Steps::new(&[Duration::ZERO, Duration::from_millis(250)]);
    let hook = rig.hook(&clock, in_process, Duration::from_secs(30));
    drop(rig.run_with(&hook, &payload("s1", SEEDED, &rig.sandbox.root())));
    let text = std::fs::read_to_string(rig.sandbox.path().join(LOG)).expect("the log");
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
        "seeds",
        "files",
        "ms",
    ];
    assert_eq!(keys, BTreeSet::from(order));
    let at = order.map(|key| {
        text.find(&format!("\"{key}\": "))
            .expect("a key in the line")
    });
    assert!(at.is_sorted(), "{text}");
    let tail = format!(
        "\"hook\": \"prompt-map\", \"event\": \"emitted\", \"reason\": \"emitted\", \
         \"session_id\": \"s1\", \"agent_id\": null, \"seeds\": [\"{INVOICE}\", \"InvoiceTotal\"], \
         \"files\": [\"{INVOICE}\""
    );
    assert!(text.starts_with("{\"ts\": 1760000000.0, "), "{text}");
    assert!(text.contains(&tail), "{text}");
    assert!(text.ends_with(", \"ms\": 250}\n"), "{text}");
}

#[test]
fn an_unset_or_empty_log_variable_writes_no_line() {
    let rig = Rig::new();
    let clock = Steps::fixed();
    let root = rig.sandbox.root();
    for (session, log) in [("s1", None), ("s2", Some(OsStr::new("")))] {
        let mut hook = rig.hook(&clock, in_process, Duration::from_secs(30));
        hook.run.log = log;
        assert!(
            !rig.run_with(&hook, &payload(session, SEEDED, &root))
                .is_empty()
        );
    }
    assert!(!rig.sandbox.path().join(LOG).exists());
}

#[test]
fn a_directory_outside_any_repository_is_silent() {
    let rig = Rig::new();
    let elsewhere = rig.sandbox.mkdir("elsewhere");
    assert_eq!(rig.run(&payload("s1", SEEDED, &elsewhere)), "");
    assert_eq!(rig.reasons(), ["no_repository"]);
}

#[test]
fn a_payload_without_a_cwd_maps_the_start_up_directory() {
    let rig = Rig::new();
    let clock = Steps::fixed();
    let root = rig.sandbox.root();
    let mut hook = rig.hook(&clock, in_process, Duration::from_secs(30));
    hook.run.cwd = Some(root.as_path());
    let stdin = json!({"session_id": "s1", "prompt": SEEDED}).to_string();
    assert!(context_of(&rig.run_with(&hook, &stdin)).contains(INVOICE));
}
