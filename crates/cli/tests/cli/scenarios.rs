//! Each scenario of crates/cli/fixtures/scenarios.json: a repository built by the same steps,
//! one command line in it, and the answer after the shared placeholders.

use std::ffi::OsString;

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
    let got = sandbox.run(&argv, &cwd, None);
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

#[test]
fn every_scenario_answers_as_the_fixture() {
    let runs = cases("scenarios", "runs");
    let mut failed = Vec::new();
    for case in runs.iter().filter(|case| buildable(case)) {
        let (got, want) = (
            with_this_cache_file(replay(case)),
            with_this_cache_file(expected(case)),
        );
        if got != want {
            let name = case["name"].as_str().unwrap_or_default();
            failed.push(format!("{name}\n got {got:?}\nwant {want:?}"));
        }
    }
    assert!(
        runs.len() >= 16,
        "the fixture holds {} scenarios",
        runs.len()
    );
    assert!(
        failed.is_empty(),
        "{} scenarios differ:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

// The reference capabilities report names its tag cache. This build reports agentless.db in the
// same directory (the D7 deviation).
fn with_this_cache_file(outcome: Outcome) -> Outcome {
    Outcome {
        out: outcome.out.replace("/<KEY>/tags.db", "/<KEY>/agentless.db"),
        ..outcome
    }
}
