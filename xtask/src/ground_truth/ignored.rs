use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use crate::suppressions::Entry;
use crate::{Failure, process};

const IGNORE: &str = "ignore";
const LISTED: &str = "listed";

#[derive(Deserialize)]
struct List {
    #[serde(rename = "rust-suites")]
    suites: BTreeMap<String, Suite>,
}

#[derive(Deserialize)]
struct Suite {
    status: String,
    testcases: BTreeMap<String, Case>,
}

#[derive(Deserialize)]
struct Case {
    ignored: bool,
}

/// Return each test that cargo nextest lists as ignored, however its attribute was written, under a deadline.
pub fn tests(root: &Path) -> Result<Vec<String>, Failure> {
    // The default target dir: CI's test step and the pre-push test hook build the same binaries there.
    let mut command = Command::new("cargo");
    command
        .args(["nextest", "list", "--workspace", "--message-format", "json"])
        .current_dir(root);
    let output = process::stdout(&mut command, process::BUILD)?;
    parse(&output).map_err(|error| Failure::Broken(format!("cargo nextest list: {error}")))
}

/// Return a problem unless the ledger's `ignore` entries count each ignored test.
pub fn problems(ignored: &[String], entries: &[Entry]) -> Vec<String> {
    let counted: usize = entries
        .iter()
        .filter(|entry| entry.lint == IGNORE)
        .map(|entry| entry.count)
        .sum();
    if counted == ignored.len() {
        return Vec::new();
    }
    let names = if ignored.is_empty() {
        "none".to_owned()
    } else {
        ignored.join(", ")
    };
    vec![format!(
        "the ignored tests that cargo nextest lists ({}: {names}) and the #[ignore] attributes that crates/suppressions.toml counts ({counted}) differ in number. A macro or an attribute that the token scan cannot read can add #[ignore], and a cfg can remove a test that the ledger counts. Run each test. To skip a test under one cfg, put the cfg on the test (#[cfg(not(miri))]), or skip at run time (if cfg!(miri) {{ return; }}). To turn a test off everywhere, write #[ignore] unconditionally and list it (lint = \"ignore\") in review",
        ignored.len()
    )]
}

fn parse(stdout: &[u8]) -> Result<Vec<String>, String> {
    let list: List = serde_json::from_slice(stdout).map_err(|error| error.to_string())?;
    let mut ignored = Vec::new();
    for (binary, suite) in list.suites {
        // A suite that nextest does not list hides its tests, ignored ones too.
        if suite.status != LISTED {
            return Err(format!(
                "the test binary {binary} is {}, not {LISTED}",
                suite.status
            ));
        }
        ignored.extend(
            suite
                .testcases
                .into_iter()
                .filter(|(_, case)| case.ignored)
                .map(|(name, _)| format!("{binary} {name}")),
        );
    }
    Ok(ignored)
}

#[cfg(test)]
mod tests {
    use super::{parse, problems};
    use crate::suppressions::Entry;

    // The shape of `cargo nextest list --message-format json` (nextest 0.9.148), cut to two suites.
    const LIST: &str = r#"{"rust-build-meta":{},"test-count":3,"rust-suites":{
        "kernel":{"package-name":"kernel","binary-id":"kernel","kind":"lib","status":"listed","testcases":{
            "id::tests::a":{"kind":"test","ignored":false,"filter-match":{"status":"matches"}},
            "id::tests::b":{"kind":"test","ignored":true,"filter-match":{"status":"mismatch","reason":"ignored"}}}},
        "xtask::bin/xtask":{"package-name":"xtask","binary-id":"xtask::bin/xtask","kind":"bin","status":"listed","testcases":{
            "repo::tests::c":{"kind":"test","ignored":true,"filter-match":{"status":"mismatch","reason":"ignored"}}}}}}"#;

    fn ignore(file: &str, count: usize) -> Entry {
        let text =
            format!("file = \"{file}\"\nlint = \"ignore\"\ncount = {count}\nreason = \"flaky\"\n");
        toml::from_str(&text).unwrap()
    }

    #[test]
    fn each_ignored_test_of_each_listed_suite_is_named() {
        assert_eq!(
            parse(LIST.as_bytes()).unwrap(),
            ["kernel id::tests::b", "xtask::bin/xtask repo::tests::c"]
        );
        let skipped = LIST.replacen(r#""status":"listed""#, r#""status":"skipped""#, 1);
        assert_eq!(
            parse(skipped.as_bytes()).unwrap_err(),
            "the test binary kernel is skipped, not listed"
        );
        assert!(parse(b"{\"rust-suites\": []}").is_err());
    }

    #[test]
    fn the_ledger_must_count_exactly_the_ignored_tests() {
        let ignored = parse(LIST.as_bytes()).unwrap();
        let mut entries = vec![ignore("crates/kernel/src/id.rs", 1)];
        assert_eq!(problems(&ignored, &entries).len(), 1);
        entries.push(ignore("xtask/src/repo.rs", 1));
        assert!(problems(&ignored, &entries).is_empty());
        entries.push(ignore("xtask/src/repo.rs", 2));
        assert!(problems(&ignored, &entries)[0].starts_with(
            "the ignored tests that cargo nextest lists (2: kernel id::tests::b, xtask::bin/xtask repo::tests::c) and the #[ignore] attributes that crates/suppressions.toml counts (4)"
        ));
        assert!(problems(&[], &entries)[0].contains("(0: none)"));
        assert!(problems(&[], &[]).is_empty());
    }
}
