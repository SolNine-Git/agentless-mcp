use std::collections::BTreeMap;
use std::time::Duration;

use json::{JsonForm, to_text};
use serde::Deserialize;
use serde_json::Value;

use super::{BlockSource, LintForm, LintReport, LintRequest, lint, lint_with};
use crate::pairs::Stored;
use crate::testing::{MANIFEST_DIR, Sandbox};
use crate::{After, Error, FormKind, Noted, PairSelection, Repo, Tags};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Expected {
    Report(String),
    Error((String, String)),
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    steps: Vec<Vec<Value>>,
    form: String,
    target: String,
    after: Option<BTreeMap<String, Value>>,
    checks: Vec<String>,
    root: Option<String>,
    cache: Option<String>,
    budget: Option<String>,
    expected: Expected,
}

fn shown(sandbox: &Sandbox, found: Noted<Result<LintReport, Error>>) -> Expected {
    let place = sandbox.path().display().to_string();
    assert_eq!(found.diagnostics, Vec::<String>::new());
    match found.value {
        Ok(report) => {
            let text = to_text(&report, JsonForm::Compact).expect("a report serializes");
            Expected::Report(text.replace(&place, "<SANDBOX>"))
        }
        Err(Error::Kind(kind)) => {
            let name = match &kind {
                kernel::Error::OperationFailed(_) => "OperationFailed",
                kernel::Error::RevisionUnresolved(_) => "RevisionUnresolved",
                other => panic!("an unexpected kind {other:?}"),
            };
            Expected::Error((
                name.to_owned(),
                kind.to_string().replace(&place, "<SANDBOX>"),
            ))
        }
        Err(other) => panic!("a defect: {other:?}"),
    }
}

// The proposed text: in memory as the hook passes it, or the file the CLI names.
fn after(sandbox: &Sandbox, spec: &BTreeMap<String, Value>, in_memory: bool) -> After {
    let text = match (spec.get("text"), spec.get("repeat"), spec.get("file")) {
        (Some(text), _, _) => text.as_str().expect("a text").to_owned(),
        (_, Some(Value::Array(repeat)), _) => {
            let line = repeat[0].as_str().expect("a line");
            let count = repeat[1].as_u64().expect("a count");
            line.repeat(usize::try_from(count).expect("a count that fits"))
        }
        (_, _, Some(file)) => {
            return After::File(sandbox.path().join(file.as_str().expect("a file name")));
        }
        _ => panic!("no after text in {spec:?}"),
    };
    if in_memory {
        return After::Text(text);
    }
    After::File(sandbox.put("after-text", text))
}

fn request(sandbox: &Sandbox, scenario: &Scenario, in_memory: bool) -> LintRequest {
    let (form, kind) = match &scenario.after {
        Some(spec) if scenario.form == "path" => {
            let after = after(sandbox, spec, in_memory);
            let path = scenario.target.clone();
            (LintForm::Proposed { path, after }, FormKind::Path)
        }
        _ => {
            let revision = scenario.target.clone();
            (LintForm::Revision { revision }, FormKind::Since)
        }
    };
    let checks = PairSelection::from_names(&scenario.checks, kind).expect("a pair selection");
    LintRequest { form, checks }
}

// A fresh tag cache stores the blocks that a re-measure gives, so a lookup that always misses
// stands in for it.
fn judged(scenario: &Scenario, in_memory: bool) -> Expected {
    let sandbox = Sandbox::new();
    for step in &scenario.steps {
        sandbox.apply(step);
    }
    let mut services = sandbox.services(sandbox.path());
    if scenario.budget.as_deref() == Some("zero") {
        services.walk_budget = Duration::ZERO;
    }
    let root = sandbox
        .path()
        .join(scenario.root.as_deref().unwrap_or("repo"));
    let repo = Repo::load(&services, &root).expect("the repository loads");
    let request = request(&sandbox, scenario, in_memory);
    let found = if scenario.cache.as_deref() == Some("fresh") {
        let mut lookup = |_: &str, _: &str, _| Stored::Miss;
        let source = BlockSource {
            cached: true,
            lookup: &mut lookup,
        };
        lint_with(&services, &repo, source, &request)
    } else {
        let tags = Tags::open(&services, &repo).value;
        let tags = tags.expect("the cache root is known");
        assert!(!tags.cached(), "{}: no tag cache", scenario.name);
        lint(&services, &repo, &tags, &request)
    };
    shown(&sandbox, found)
}

#[test]
fn each_proposed_scenario_gives_the_reference_report_from_a_file_and_from_memory() {
    let scenarios: Vec<Scenario> = io::testing::cases(MANIFEST_DIR, "lint", "proposed");
    for scenario in &scenarios {
        assert_eq!(
            judged(scenario, false),
            scenario.expected,
            "{}",
            scenario.name
        );
        let from_file = scenario
            .after
            .as_ref()
            .is_some_and(|spec| spec.contains_key("file"));
        if !from_file {
            assert_eq!(
                judged(scenario, true),
                scenario.expected,
                "{} in memory",
                scenario.name
            );
        }
    }
}

#[test]
fn each_revision_scenario_gives_the_reference_report() {
    let scenarios: Vec<Scenario> = io::testing::cases(MANIFEST_DIR, "lint", "revision");
    for scenario in &scenarios {
        assert_eq!(
            judged(scenario, false),
            scenario.expected,
            "{}",
            scenario.name
        );
    }
}

#[test]
fn a_run_without_clone_delta_never_reads_a_stored_block() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "x = 1\n")]);
    sandbox.put("repo/lib.py", "y = 2\n");
    let services = sandbox.services(sandbox.path());
    let repo = Repo::load(&services, &root).expect("the repository loads");
    let mut lookup = |path: &str, _: &str, _| panic!("{path}: no clone_delta, no index");
    let source = BlockSource {
        cached: true,
        lookup: &mut lookup,
    };
    let request = LintRequest {
        form: LintForm::Revision {
            revision: "HEAD".to_owned(),
        },
        checks: PairSelection::edited_files(),
    };

    let found = lint_with(&services, &repo, source, &request).value;
    let found = found.expect("a report");

    let checks: Vec<&str> = found.candidates[0]
        .findings
        .iter()
        .map(|row| row.finding.check)
        .collect();
    assert_eq!(checks, ["contract_drift"]);
}

#[test]
fn a_relative_after_file_with_an_unread_cwd_gives_the_new_side_row() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "x = 1\n")]);
    let mut services = sandbox.services(sandbox.path());
    let unread = crate::EnvConfig::with_unread_cwd([], &std::io::Error::from_raw_os_error(2));
    services.cwd = Err(unread.cwd().expect_err("the cwd is unread").clone());
    let repo = Repo::load(&services, &root).expect("an absolute root needs no cwd");
    let tags = Tags::open(&services, &repo).value;
    let tags = tags.expect("the cache root is known");
    let request = LintRequest {
        form: LintForm::Proposed {
            path: "app.py".to_owned(),
            after: After::File(std::path::PathBuf::from("after.py")),
        },
        checks: PairSelection::edited_files(),
    };

    let found = lint(&services, &repo, &tags, &request).value;
    let found = found.expect("a report");

    let rows = &found.candidates[0].findings;
    let messages: Vec<&str> = rows
        .iter()
        .map(|row| row.finding.message.as_str())
        .collect();
    assert_eq!(
        messages,
        [
            "not checked: the proposed text for app.py was not read (unreadable: No such file or \
             directory), so no pair check compared this file."
        ]
    );
}
