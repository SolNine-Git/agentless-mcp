// The index scenarios of scripts/store_index_fixtures.py, replayed step by step.

use serde::Deserialize;
use serde_json::Value;

use super::{run, services};
use crate::IndexReport;
use crate::testing::{MANIFEST_DIR, Sandbox};

#[derive(Deserialize)]
struct Scenario {
    name: String,
    steps: Vec<Vec<Value>>,
    runs: Vec<Run>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Run {
    indexed: usize,
    reused: usize,
    pruned: usize,
    skipped: usize,
    errors: usize,
    files: u64,
    tags: u64,
    imports: u64,
    refs: u64,
    generation: String,
    failures: Vec<(String, String)>,
    skipped_files: Vec<(String, String)>,
}

// The database path differs by design (D10) and is not in the run.
fn run_of(report: IndexReport) -> Run {
    Run {
        indexed: report.indexed,
        reused: report.reused,
        pruned: report.pruned,
        skipped: report.skipped(),
        errors: report.errors(),
        files: report.files,
        tags: report.tags,
        imports: report.imports,
        refs: report.refs,
        generation: report.generation,
        failures: pairs(report.failures.into_iter().map(|f| (f.path, f.reason))),
        skipped_files: pairs(report.skipped_files.into_iter().map(|s| (s.path, s.reason))),
    }
}

fn pairs(found: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    found.collect()
}

#[test]
fn each_scenario_run_gives_the_reference_report() {
    let scenarios: Vec<Scenario> =
        text::testing::cases(MANIFEST_DIR, "index/scenarios", "scenarios");
    for scenario in &scenarios {
        let sandbox = Sandbox::new();
        // Only a scenario that changes a mode needs the mode to bind, which it does not for root.
        let changes_modes = (scenario.steps.iter()).any(|step| step[0].as_str() == Some("chmod"));
        if changes_modes && !sandbox.modes_bind() {
            continue;
        }
        let services = services(&sandbox);
        let mut expected = scenario.runs.iter();
        for step in &scenario.steps {
            let kind = step[0].as_str().expect("a step kind");
            let place = step[1].as_str().expect("a step place");
            match kind {
                "index" | "index_force" => {
                    let want = expected.next().expect("a reference run");
                    let root = sandbox.path().join(place);
                    let noted = run(&services, &root, kind == "index_force");
                    let report = noted.value.expect("the index run succeeds");
                    assert_eq!(&run_of(report), want, "{}", scenario.name);
                }
                "chmod" => {
                    let mode = step[2].as_u64().expect("a mode");
                    sandbox.chmod(place, u32::try_from(mode).expect("a mode"));
                }
                _ => sandbox.apply(step),
            }
        }
        let left = expected.next();
        assert!(left.is_none(), "{}: a run was not replayed", scenario.name);
    }
}
