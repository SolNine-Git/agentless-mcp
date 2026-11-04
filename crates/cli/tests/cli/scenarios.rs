//! Each scenario of the scenario fixtures: a repository built by the same steps, one command line
//! in it, and the answer after the shared placeholders. crates/cli/fixtures/scenarios.json holds
//! lint and capabilities; crates/cli/fixtures/read/scenarios.json holds the read subcommands.

use std::ffi::OsString;

use prompts::envelope::RECEIPT_HEADER;
use serde_json::Value;

use crate::common::{Outcome, Sandbox, cases, expected, normalised, words};

fn replay(case: &Value) -> Outcome {
    let sandbox = Sandbox::new();
    let steps = case["steps"].as_array().expect("a step list");
    for step in steps {
        sandbox.step(step.as_array().expect("a step"));
    }
    let root = sandbox.path().display().to_string();
    let argv: Vec<OsString> = words(&case["argv"])
        .into_iter()
        .map(|word| OsString::from(word.replace("<SANDBOX>", &root)))
        .collect();
    let cwd = sandbox
        .path()
        .join(case["cwd"].as_str().expect("a working directory"));
    let mut stdin = case["stdin"].as_str().unwrap_or_default().as_bytes();
    let got = sandbox.run_with_stdin(&argv, &cwd, None, &mut stdin);
    Outcome {
        out: normalised(&got.out, sandbox.path()),
        err: normalised(&got.err, sandbox.path()),
        code: got.code,
    }
}

// APFS refuses a file name that is not UTF-8, so on macOS a scenario that writes one is left out.
fn buildable(case: &Value) -> bool {
    let steps = case["steps"].as_array().expect("a step list");
    !cfg!(target_os = "macos") || !steps.iter().any(|step| step[0] == "latin")
}

// Replays every scenario of `file`, compares each answer after `seen`, and fails with each one
// that differs.
fn every_scenario_answers_as(file: &str, at_least: usize, seen: fn(Outcome) -> Outcome) {
    let runs = cases(file, "runs");
    let mut failed = Vec::new();
    for case in runs.iter().filter(|case| buildable(case)) {
        let (got, want) = (seen(replay(case)), seen(expected(case)));
        if got != want {
            let name = case["name"].as_str().unwrap_or_default();
            failed.push(format!("{name}\n got {got:?}\nwant {want:?}"));
        }
    }
    assert!(
        runs.len() >= at_least,
        "{file} holds {} scenarios",
        runs.len()
    );
    assert!(
        failed.is_empty(),
        "{} scenarios differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn every_scenario_answers_as_the_fixture() {
    every_scenario_answers_as("scenarios", 21, with_this_cache_file);
}

// The reference capabilities report names its tag cache. This build reports agentless.db in the
// same directory (the D7 deviation).
fn with_this_cache_file(outcome: Outcome) -> Outcome {
    Outcome {
        out: this_cache_file(&outcome.out),
        ..outcome
    }
}

fn this_cache_file(text: &str) -> String {
    text.replace("/<KEY>/tags.db", "/<KEY>/agentless.db")
}

// The receipt's cache field names the cache file of each build, and warmup names the grammar
// cache, so both sides show a placeholder for each.
#[test]
fn every_read_scenario_answers_as_the_fixture() {
    every_scenario_answers_as("read/scenarios", 51, without_cache_field);
}

#[test]
fn every_map_scenario_answers_as_the_fixture() {
    every_scenario_answers_as("read/map_scenarios", 27, without_cache_field);
}

#[test]
fn every_symbol_view_scenario_answers_as_the_fixture() {
    every_scenario_answers_as("read_symbols/scenarios", 70, without_cache_field);
}

fn without_cache_field(outcome: Outcome) -> Outcome {
    Outcome {
        out: masked_lines(&outcome.out),
        ..outcome
    }
}

// The diagram and the export put their receipt on stderr, and index names its cache file.
#[test]
fn every_graph_view_scenario_answers_as_the_fixture() {
    every_scenario_answers_as("read_graph/scenarios", 74, without_cache_names);
}

fn without_cache_names(outcome: Outcome) -> Outcome {
    let Outcome { out, err, code } = outcome;
    Outcome {
        out: this_cache_file(&masked_lines(&out)),
        err: masked_lines(&err),
        code,
    }
}

// The DURATION mask of the generator: each verdict record's run time is a placeholder.
#[test]
fn every_write_scenario_answers_as_the_fixture() {
    every_scenario_answers_as("write_scenarios", 81, without_durations);
}

fn without_durations(outcome: Outcome) -> Outcome {
    let Outcome { out, err, code } = outcome;
    Outcome {
        out: duration_masked(&out),
        err: duration_masked(&err),
        code,
    }
}

// Each `"duration": ` followed by a number reads `"duration": <DURATION>`.
fn duration_masked(text: &str) -> String {
    const FIELD: &str = "\"duration\": ";
    let mut done = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(FIELD) {
        let (head, tail) = rest.split_at(at + FIELD.len());
        done.push_str(head);
        let number = tail
            .find(|c: char| !(c.is_ascii_digit() || "-+.e".contains(c)))
            .unwrap_or(tail.len());
        let unsigned = tail.strip_prefix('-').unwrap_or(tail);
        if unsigned.starts_with(|c: char| c.is_ascii_digit()) {
            done.push_str("<DURATION>");
            rest = &tail[number..];
        } else {
            rest = tail;
        }
    }
    done.push_str(rest);
    done
}

fn masked_lines(text: &str) -> String {
    let lines: Vec<String> = text.split('\n').map(masked).collect();
    lines.join("\n")
}

fn masked(line: &str) -> String {
    if line.starts_with(RECEIPT_HEADER)
        && let Some((head, _)) = line.split_once("   cache: ")
    {
        return format!("{head}   cache: <CACHE>");
    }
    let field = "    \"cache\": \"";
    if line.starts_with(field) && line.ends_with("\",") {
        return format!("{field}<CACHE>\",");
    }
    if line.starts_with("pack ")
        && let Some((release, _)) = line.split_once("  cache ")
    {
        return format!("{release}  cache <CACHE>");
    }
    line.to_owned()
}
