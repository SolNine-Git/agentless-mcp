use std::panic::{self, AssertUnwindSafe};

use facts::Facts;
use git::{RepoFile, Snapshot};
use kernel::Stopped;
use store::{FactSource, FileText, Served};
use syntax::Unparsable;

use super::{RepoFacts, line_count, scan};
use crate::clock::{assert_spent, spent};
use crate::endpoints::scanned::Scanned;
use crate::testing::Sandbox;
use crate::{Reading, Repo};

fn listed(paths: &[&str]) -> Vec<RepoFile> {
    paths
        .iter()
        .map(|path| RepoFile {
            path: (*path).to_owned(),
            size: 0,
        })
        .collect()
}

fn parsed(text: &FileText) -> Served<Result<Facts, Unparsable>> {
    FactSource::parsing().facts(text, None)
}

// Each file's path, line count and symbol count, and each skipped path with its reason.
type Summary = (Vec<(String, u32, usize)>, Vec<(String, String)>);

fn summary(facts: &RepoFacts) -> Summary {
    let files = facts.files.iter();
    let files = files.map(|file| (file.path.clone(), file.line_count, file.facts.symbols.len()));
    let skipped = facts.skipped.iter();
    let skipped = skipped.map(|entry| (entry.path.clone(), entry.reason.clone()));
    (files.collect(), skipped.collect())
}

#[test]
fn only_files_in_a_language_are_scanned_and_the_config_is_not() {
    let sandbox = Sandbox::new();
    sandbox.put("a.py", "def f():\n    return 1\n");
    sandbox.put("notes.txt", "text\n");
    sandbox.put(".agentless-mcp.json", "{}\n");
    sandbox.put("sub/.agentless-mcp.json", "{}\n");
    sandbox.put("big.py", "x = 1\n".repeat(200_000));
    let files = listed(&[
        "a.py",
        "notes.txt",
        ".agentless-mcp.json",
        "sub/.agentless-mcp.json",
    ]);
    let files = [files, listed(&["big.py"])].concat();
    let scanned = scan(sandbox.path(), files, 4, &parsed, None);
    let (files, skipped) = summary(scanned.value.as_ref().expect("no deadline stops it"));
    assert_eq!(
        files,
        [
            ("a.py".to_owned(), 3, 1),
            ("sub/.agentless-mcp.json".to_owned(), 2, 0)
        ]
    );
    let reason = "skipped: 1200000 bytes exceeds the per-file cap of 1000000 bytes";
    assert_eq!(skipped, [("big.py".to_owned(), reason.to_owned())]);
    assert!(scanned.diagnostics.is_empty());
}

#[test]
fn the_line_count_counts_the_parts_between_line_feeds() {
    assert_eq!(line_count(""), 1);
    assert_eq!(line_count("a"), 1);
    assert_eq!(line_count("a\n"), 2);
    assert_eq!(line_count("a\r\nb\rc\n\n"), 4);
}

fn many(sandbox: &Sandbox, count: usize) -> Vec<RepoFile> {
    let paths: Vec<String> = (0..count).map(|index| format!("m{index:02}.py")).collect();
    for (index, path) in paths.iter().enumerate() {
        sandbox.put(path, format!("def f{index}():\n    return {index}\n"));
    }
    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
    listed(&paths)
}

// Failure injection: one file's parse panics on whichever thread takes it.
#[test]
fn a_panic_in_one_file_reaches_the_caller_with_its_payload() {
    let sandbox = Sandbox::new();
    let files = many(&sandbox, 24);
    let panicking = |text: &FileText| {
        assert_ne!(text.path, "m17.py", "the parse of m17.py");
        parsed(text)
    };
    let caught = panic::catch_unwind(AssertUnwindSafe(|| {
        scan(sandbox.path(), files, 4, &panicking, None)
    }));
    let payload = caught.expect_err("the panic reaches the caller");
    let message = payload
        .downcast_ref::<String>()
        .expect("a formatted message");
    assert!(message.contains("the parse of m17.py"), "{message}");
}

// Failure injection: when the OS refuses every helper thread, the caller scans each file alone,
// which is the scan on one thread.
#[test]
fn a_scan_without_helpers_equals_the_scan_with_them() {
    let sandbox = Sandbox::new();
    let files = many(&sandbox, 40);
    let unstopped = |scanned: Result<RepoFacts, Stopped>| scanned.expect("no deadline stops it");
    let alone = scan(sandbox.path(), files.clone(), 1, &parsed, None).map(unstopped);
    let helped = scan(sandbox.path(), files, 8, &parsed, None).map(unstopped);
    assert_eq!(alone.value.files, helped.value.files);
    assert_eq!(alone.value.skipped, helped.value.skipped);
    assert_eq!(alone.value.files.len(), 40);
}

#[test]
fn the_scan_of_a_repository_walks_it_and_reads_through_the_source() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo(
        "repo",
        &[("b.py", "import a\n"), ("a.py", "def g():\n    pass\n")],
    );
    let services = sandbox.services(&root);
    let repo = Repo::load(&services, &root).unwrap_or_else(|error| panic!("{error}"));
    let snapshot = Snapshot {
        head_sha: None,
        tree_oid: None,
        dirty_count: None,
        note: String::new(),
    };
    let source = FactSource::parsing();
    let reading = Reading {
        repo: &repo,
        snapshot: &snapshot,
        facts: &source,
        deadline: None,
    };
    let loaded = RepoFacts::load(&services, &reading);
    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
    let facts = loaded.value.unwrap_or_else(|error| panic!("{error}"));
    let (files, skipped) = summary(&facts);
    assert_eq!(
        files,
        [("a.py".to_owned(), 3, 1), ("b.py".to_owned(), 2, 0)]
    );
    assert!(skipped.is_empty());
    assert_eq!(facts.files[1].facts.imports.len(), 1);
}

#[test]
fn the_scan_stops_under_a_spent_deadline() {
    let scanned = Scanned::new(&[("a.py", "def f():\n    return 1\n")]);
    let deadline = spent();
    let reading = scanned.reading().with_deadline(&deadline);

    let loaded = RepoFacts::load(&scanned.services, &reading);

    let error = loaded.value.expect_err("the deadline stops the scan");
    assert_spent(&error);
}

// A file in no language is never read, and its check is in the serial filter.
#[test]
fn a_walk_with_no_file_in_a_language_still_stops_under_a_spent_deadline() {
    let sandbox = Sandbox::new();
    sandbox.put("notes.txt", "text\n");
    let deadline = spent();

    let scanned = scan(
        sandbox.path(),
        listed(&["notes.txt"]),
        4,
        &parsed,
        Some(&deadline),
    );

    let stopped = scanned.value.map(|facts| facts.files.len());
    assert!(matches!(stopped, Err(Stopped::Spent { .. })), "{stopped:?}");
}
