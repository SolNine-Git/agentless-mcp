use std::ffi::OsStr;
use std::fs;
use std::num::NonZeroUsize;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    Commit, CommitId, HISTORY, HistoryFailure, LineRange, MAX_HISTORY_OUTPUT_BYTES, PROBE, SpanLog,
    SpanLogError, SpanQuery, parse_log, shallow_answer,
};
use crate::recipe::{FIXTURE, Repos};
use crate::run::classify;
use crate::testing::{Scratch, cases, latin, path_before};
use crate::{Git, GitOutcome};

// A body longer than this is compared through its length and digest, as the fixture stores it.
const SHOWN_BODY_CHARS: usize = 256;
// One log record of exactly 80 bytes, so the cap holds a whole number of them.
const RECORD: &str = concat!(
    "0123456789abcdef0123456789abcdef01234567\\000",
    "2026-01-01T00:00:00Z\\000s\\000bbbbbbbbbbbbbbb\\000",
);
const RECORD_BYTES: usize = 80;
const SOME_ID: &str = "0123456789abcdef0123456789abcdef01234567";
// The reference's line for a directory outside every repository. Git names the mount point
// instead when its search stops at a file-system boundary, as on a tmpfs temporary directory.
const REFERENCE_DISCOVERY_LINE: &str =
    "fatal: not a git repository (or any of the parent directories): .git";

fn id(text: &str) -> CommitId {
    CommitId::parse(text).unwrap_or_else(|| panic!("{text:?} is not a full id"))
}

fn class_name(failure: HistoryFailure) -> &'static str {
    match failure {
        HistoryFailure::NoPath => "no_path",
        HistoryFailure::SpanBeyondHead => "span_beyond_head",
        HistoryFailure::NoGit => "no_git",
        HistoryFailure::Other => "other",
    }
}

fn shown_body(body: &str) -> Value {
    let chars = body.chars().count();
    if chars <= SHOWN_BODY_CHARS {
        return json!(body);
    }
    let digest = kernel::Key::new(Sha256::digest(body.as_bytes()).into()).to_string();
    json!({"chars": chars, "sha256": digest})
}

fn rows(commits: &[Commit]) -> Value {
    let rows: Vec<Value> = commits
        .iter()
        .map(|commit| {
            json!([
                commit.sha,
                commit.authored,
                commit.subject,
                shown_body(&commit.body)
            ])
        })
        .collect();
    json!(rows)
}

fn parsed(result: Result<Vec<Commit>, String>) -> Value {
    match result {
        Ok(commits) => json!({"commits": rows(&commits)}),
        Err(detail) => json!({"malformed": detail}),
    }
}

fn logged(repos: &Repos, result: Result<SpanLog, SpanLogError>) -> Value {
    match result {
        Ok(log) => json!({"commits": rows(&log.commits), "capped": log.capped}),
        Err(SpanLogError::Unanswered(note)) => json!({
            "unanswered": repos.unrooted(&note),
            "class": class_name(HistoryFailure::of(&note)),
        }),
        Err(SpanLogError::Malformed(detail)) => json!({"malformed": detail}),
    }
}

fn answered(repos: &Repos, result: Result<bool, String>) -> Value {
    match result {
        Ok(flag) => json!(flag),
        Err(note) => json!({"unknown": repos.unrooted(&note)}),
    }
}

// The first line git prints in `dir`, which no repository encloses, with the code's own command
// prefix and environment. Each directory below `dir` on its file system prints the same line.
fn discovery_line(git: &Git, dir: &std::path::Path) -> String {
    let spec = git.spec(dir, "rev-parse", &[OsStr::new("--git-dir")], PROBE);
    let outcome = io::run(&spec);
    let io::Outcome::Exited { status, stderr, .. } = &outcome else {
        panic!("git rev-parse in {}: {outcome:?}", dir.display());
    };
    let text = String::from_utf8_lossy(stderr);
    let refused = |line: &&str| line.contains("not a git repository");
    match text.lines().next().filter(refused) {
        Some(line) if io::exit_code(*status) == 128 => line.to_owned(),
        _ => panic!("a repository encloses {}: {status}: {text}", dir.display()),
    }
}

// `expected` with the reference's discovery line replaced by `line` in each text field. Every
// other byte stays as the reference recorded it.
fn discovered(expected: Value, line: &str) -> Value {
    let Value::Object(fields) = expected else {
        return expected;
    };
    let replaced = |value: Value| match value {
        Value::String(text) => Value::String(text.replace(REFERENCE_DISCOVERY_LINE, line)),
        other => other,
    };
    let fields = fields
        .into_iter()
        .map(|(key, value)| (key, replaced(value)));
    Value::Object(fields.collect())
}

fn query<'a>(
    path: &'a str,
    lines: (u32, u32),
    count: usize,
    revision: &'a CommitId,
) -> SpanQuery<'a> {
    SpanQuery {
        path,
        lines: LineRange::new(lines.0, lines.1).expect("the fixture's lines are a range"),
        revision,
        max_count: NonZeroUsize::new(count).expect("the fixture's count is positive"),
    }
}

#[test]
fn parse_log_matches_the_reference_cases() {
    let all: Vec<(String, bool, Value)> = cases(FIXTURE, "parse_log");
    assert!(!all.is_empty());
    for (text, capped, expected) in all {
        let got = parsed(parse_log(&text, capped));
        assert_eq!(got, expected, "{text:?} capped {capped}");
    }
}

// Every new call decodes git's stdout with from_utf8_lossy, so this pins its replacements.
#[test]
fn lossy_decoding_matches_the_reference_cases() {
    let all: Vec<(String, String)> = cases(FIXTURE, "decode");
    assert!(!all.is_empty());
    for (raw, expected) in all {
        let raw = latin(&raw);
        assert_eq!(String::from_utf8_lossy(&raw), expected, "{raw:x?}");
    }
}

#[test]
fn each_note_is_classified_as_the_reference_says() {
    let all: Vec<(String, String)> = cases(FIXTURE, "classify");
    assert!(!all.is_empty());
    for (note, expected) in all {
        assert_eq!(class_name(HistoryFailure::of(&note)), expected, "{note:?}");
    }
}

#[test]
fn a_commit_id_is_a_full_lowercase_hex_id_and_nothing_else() {
    let sha1 = "08ec32abb9b149de9d10310c24bd12bd1fe8d874";
    let sha256 = "a".repeat(64);
    assert_eq!(
        CommitId::parse(sha1).map(|id| id.to_string()),
        Some(sha1.to_owned())
    );
    assert_eq!(
        CommitId::parse(&sha256).map(|id| id.as_str().len()),
        Some(64)
    );
    let refused = [
        "-x".to_owned(),
        "--output=/tmp/x".to_owned(),
        sha1.to_uppercase(),
        sha1[..8].to_owned(),
        "HEAD".to_owned(),
        String::new(),
        format!("-{}", &sha1[1..]),
        "a".repeat(41),
        "g".repeat(40),
    ];
    for text in refused {
        assert_eq!(CommitId::parse(&text), None, "{text:?}");
    }
}

#[test]
fn a_line_range_starts_at_one_and_never_ends_before_it_starts() {
    assert_eq!(LineRange::new(0, 1), None);
    assert_eq!(LineRange::new(3, 2), None);
    let range = LineRange::new(2, 2).expect("one line is a range");
    assert_eq!((range.start(), range.end()), (2, 2));
}

#[test]
fn the_shallow_answer_quotes_at_most_80_characters_of_anything_else() {
    assert_eq!(shallow_answer(" true\n"), Ok(true));
    assert_eq!(shallow_answer("false"), Ok(false));
    let long = format!("it's {}", "x".repeat(100));
    let shown: String = long.chars().take(80).collect();
    let expected = format!("git rev-parse printed \"{shown}\"");
    assert_eq!(shallow_answer(&long), Err(expected));
}

#[test]
fn each_span_log_over_real_repositories_matches_the_reference() {
    type Case = (String, String, u32, u32, usize, String, Value);
    let repos = Repos::build();
    let git = repos.scratch.git();
    let line = discovery_line(&git, &repos.scratch.root);
    let all: Vec<Case> = cases(FIXTURE, "log_span");
    assert!(!all.is_empty());
    for (repo, path, start, end, count, revision, expected) in all {
        let revision = id(&revision);
        let span = query(&path, (start, end), count, &revision);
        let got = logged(&repos, git.log_span(&repos.path(&repo), &span));
        let expected = discovered(expected, &line);
        assert_eq!(got, expected, "{repo} {path}:{start}-{end} at {revision}");
    }
}

#[test]
fn each_diff_answer_over_real_repositories_matches_the_reference() {
    let repos = Repos::build();
    let git = repos.scratch.git();
    let all: Vec<(String, String, String, Value)> = cases(FIXTURE, "differs");
    assert!(!all.is_empty());
    for (repo, path, revision, expected) in all {
        let revision = id(&revision);
        let got = answered(&repos, git.differs(&repos.path(&repo), &path, &revision));
        assert_eq!(got, expected, "{repo} {path} at {revision}");
    }
}

#[test]
fn each_shallow_answer_over_real_repositories_matches_the_reference() {
    let repos = Repos::build();
    let git = repos.scratch.git();
    let line = discovery_line(&git, &repos.scratch.root);
    let all: Vec<(String, Value)> = cases(FIXTURE, "shallow");
    assert!(!all.is_empty());
    for (repo, expected) in all {
        let got = answered(&repos, git.is_shallow(&repos.path(&repo)));
        let expected = discovered(expected, &line);
        assert_eq!(got, expected, "{repo}");
    }
}

// Runs one history call, named as the fixture names it, against a git that prints nothing.
fn run_call(git: &Git, root: &std::path::Path, call: &str, input: &Value) {
    let text = |index: usize| input[index].as_str().expect("a text input").to_owned();
    let number = |index: usize| input[index].as_u64().expect("a number input");
    match call {
        "log_span" => {
            let start = u32::try_from(number(1)).expect("a small line");
            let end = u32::try_from(number(2)).expect("a small line");
            let count = usize::try_from(number(3)).expect("a small count");
            let (path, revision) = (text(0), id(&text(4)));
            let span = query(&path, (start, end), count, &revision);
            // An empty stdout parses to no commits.
            let commits = git.log_span(root, &span).map(|log| log.commits);
            assert_eq!(commits, Ok(Vec::new()));
        }
        "differs" => assert_eq!(git.differs(root, &text(0), &id(&text(1))), Ok(false)),
        "is_shallow" => assert!(git.is_shallow(root).is_err()),
        other => panic!("no history call is named {other}"),
    }
}

#[test]
fn each_history_call_sends_the_reference_arguments_after_the_directory() {
    let scratch = Scratch::new();
    let bin = scratch.fake_git("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$ARGV_LOG\"\n", 0o755);
    let log = scratch.root.join("argv.log");
    let env = [("PATH", bin.as_os_str()), ("ARGV_LOG", log.as_os_str())];
    let git = Git::new(scratch.env_with(&env));
    let root = scratch.root.as_path();
    let all: Vec<(String, Value, Vec<String>)> = cases(FIXTURE, "argv");
    let history = all.iter().filter(|(call, _, _)| call != "churn");
    for (call, input, expected) in history {
        run_call(&git, root, call, input);
        let argv = fs::read_to_string(&log).expect("the fake git wrote its argv");
        let lines: Vec<&str> = argv.lines().collect();
        let directory = root.to_str().expect("a UTF-8 scratch path");
        let wanted: Vec<&str> = ["-C", directory]
            .into_iter()
            .chain(expected.iter().map(String::as_str))
            .collect();
        assert!(lines.ends_with(&wanted), "{call}: {lines:?}");
    }
}

// The cap tests below tie each call to its bounds. This pins the note each deadline gives.
#[test]
fn each_history_call_names_its_deadline_in_the_timeout_note() {
    let calls = [
        ("log", HISTORY, "git log timed out after 30.0s"),
        ("diff", PROBE, "git diff timed out after 5.0s"),
        ("rev-parse", PROBE, "git rev-parse timed out after 5.0s"),
    ];
    for (subcommand, limits, expected) in calls {
        let outcome = classify(subcommand, limits, io::Outcome::TimedOut);
        let GitOutcome::Unfinished(unfinished) = outcome else {
            panic!("{subcommand}: a deadline is unfinished, not {outcome:?}");
        };
        assert_eq!(unfinished.to_string(), expected);
        assert_eq!(HistoryFailure::of(expected), HistoryFailure::Other);
    }
}

// A git whose stdout is what the shell line `stream` prints, with FAKE_BYTES set per call.
fn printing(scratch: &Scratch, stream: &str) -> impl Fn(usize) -> Git {
    let path = path_before(scratch.fake_git(&format!("#!/bin/sh\n{stream}\n"), 0o755));
    let base = scratch.env();
    move |bytes: usize| {
        let count = bytes.to_string();
        let kept = base.iter().filter(|(name, _)| name != "PATH").cloned();
        let added = [
            ("PATH".into(), path.clone()),
            ("FAKE_BYTES".into(), count.into()),
        ];
        Git::new(kept.chain(added))
    }
}

#[test]
fn the_span_log_keeps_the_whole_records_under_its_cap() {
    assert_eq!(MAX_HISTORY_OUTPUT_BYTES % RECORD_BYTES, 0);
    let scratch = Scratch::new();
    let stream = format!("while :; do printf '{RECORD}'; done | head -c \"$FAKE_BYTES\"");
    let git_printing = printing(&scratch, &stream);
    let revision = id(SOME_ID);
    let span = query("a.py", (1, 1), 1, &revision);
    let records = MAX_HISTORY_OUTPUT_BYTES / RECORD_BYTES;
    let whole = git_printing(MAX_HISTORY_OUTPUT_BYTES).log_span(&scratch.root, &span);
    let whole = whole.expect("whole records parse");
    assert_eq!((whole.commits.len(), whole.capped), (records, false));
    let cut = git_printing(MAX_HISTORY_OUTPUT_BYTES + 1).log_span(&scratch.root, &span);
    let cut = cut.expect("a cut log keeps its whole records");
    assert_eq!((cut.commits.len(), cut.capped), (records, true));
}

#[test]
fn the_diff_answer_is_unknown_with_no_note_over_its_cap() {
    let scratch = Scratch::new();
    let git_printing = printing(&scratch, "head -c \"$FAKE_BYTES\" /dev/zero");
    let revision = id(SOME_ID);
    let at_cap = git_printing(PROBE.cap).differs(&scratch.root, "a.py", &revision);
    assert_eq!(at_cap, Ok(false));
    let over = git_printing(PROBE.cap + 1).differs(&scratch.root, "a.py", &revision);
    assert_eq!(over, Err(String::new()));
}

// "true", FAKE_BYTES spaces, then "x": the answer reads the "x" only when it is inside the cap.
#[test]
fn the_shallow_answer_reads_no_byte_past_its_cap() {
    let scratch = Scratch::new();
    let stream = "printf true; head -c \"$FAKE_BYTES\" /dev/zero | tr '\\0' ' '; printf x";
    let git_printing = printing(&scratch, stream);
    let fill = PROBE.cap - "true".len();
    let inside = git_printing(fill - 1).is_shallow(&scratch.root);
    assert!(inside.is_err_and(|note| note.starts_with("git rev-parse printed 'true ")));
    let past = git_printing(fill).is_shallow(&scratch.root);
    assert_eq!(past, Ok(true));
}
