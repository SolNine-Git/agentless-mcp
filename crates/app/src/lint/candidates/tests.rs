use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use json::{JsonForm, to_text};
use serde::Deserialize;
use serde_json::Value;
use store::FactSource;

use super::inputs::stem;
use super::manifests::read_declared_dependencies;
use super::{
    CandidatesRequest, CheckSelection, LintCandidateInput, candidates_with, load_lint_candidates,
    load_lint_diff,
};
use crate::lint::BlockSource;
use crate::pairs::Stored;
use crate::testing::{MANIFEST_DIR, Sandbox, buildable_cases};
use crate::{Error, LintReport, Noted, Reading, Repo, Services};

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
    target: String,
    form: Option<String>,
    checks: Option<Vec<String>>,
    root: Option<String>,
    budget: Option<String>,
    expected: Expected,
    diagnostics: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Declared {
    packages: Vec<String>,
    sources: Vec<String>,
    warnings: Vec<String>,
    requires_python: String,
}

#[derive(Debug, Deserialize)]
struct ManifestCase {
    name: String,
    steps: Vec<Vec<Value>>,
    expected: Declared,
}

#[derive(Debug, Deserialize)]
struct Golden {
    candidates: Vec<(String, String)>,
    boundary_globs: Vec<String>,
    files: BTreeMap<String, String>,
}

// The error kinds by the reference's class names.
fn kind_name(error: &kernel::Error) -> &'static str {
    match error {
        kernel::Error::OperationFailed(_) => "OperationFailed",
        kernel::Error::InputUnreadable(_) => "InputUnreadable",
        kernel::Error::SecurityRefusal(_) => "SecurityRefusal",
        other => panic!("an unexpected kind {other:?}"),
    }
}

fn refused(error: Error, sandbox: &Sandbox) -> Expected {
    let place = sandbox.path().display().to_string();
    match error {
        Error::Kind(kind) => {
            let message = kind.to_string().replace(&place, "<SANDBOX>");
            Expected::Error((kind_name(&kind).to_owned(), message))
        }
        other => panic!("a defect: {other:?}"),
    }
}

// The inputs load first, then the selection, as the reference evaluates them.
fn request(
    services: &Services,
    sandbox: &Sandbox,
    scenario: &Scenario,
) -> Result<CandidatesRequest, Error> {
    let target = sandbox.path().join(&scenario.target);
    let candidates = match scenario.form.as_deref() {
        Some("diff") => vec![load_lint_diff(services, &target)?],
        _ => load_lint_candidates(services, &target)?,
    };
    let checks = match &scenario.checks {
        Some(names) => CheckSelection::from_names(names)
            .map_err(|message| Error::Kind(kernel::Error::OperationFailed(message)))?,
        None => CheckSelection::all(),
    };
    Ok(CandidatesRequest { candidates, checks })
}

// A run with no tag cache measures every index block in memory.
fn linted(
    services: &Services,
    repo: &Repo,
    request: &CandidatesRequest,
    lookup: &mut dyn FnMut(&str, &str, syntax::Language) -> Stored,
) -> Noted<Result<LintReport, Error>> {
    let enclosing = services.git.root(&repo.root);
    let snapshot = services.git.snapshot(&repo.root, enclosing.as_deref());
    let facts = FactSource::parsing();
    let reading = Reading {
        repo,
        snapshot: &snapshot,
        facts: &facts,
    };
    let source = BlockSource {
        cached: false,
        lookup,
    };
    candidates_with(services, &reading, source, request)
}

fn compact(report: &LintReport) -> String {
    to_text(report, JsonForm::Compact).expect("a report serializes")
}

fn judged(scenario: &Scenario) -> (Expected, Vec<String>) {
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
    let request = match request(&services, &sandbox, scenario) {
        Ok(request) => request,
        Err(error) => return (refused(error, &sandbox), Vec::new()),
    };
    let mut miss = |_: &str, _: &str, _| Stored::Miss;
    let Noted { value, diagnostics } = linted(&services, &repo, &request, &mut miss);
    let place = sandbox.path().display().to_string();
    let found = match value {
        Ok(report) => Expected::Report(compact(&report).replace(&place, "<SANDBOX>")),
        Err(error) => refused(error, &sandbox),
    };
    (found, diagnostics)
}

#[test]
fn each_candidate_scenario_gives_the_reference_report() {
    let scenarios = buildable_cases("write/lint", "candidates", |case: &Scenario| &case.steps);
    for scenario in &scenarios {
        let (found, diagnostics) = judged(scenario);
        assert_eq!(found, scenario.expected, "{}", scenario.name);
        assert_eq!(
            diagnostics, scenario.diagnostics,
            "{} warnings",
            scenario.name
        );
    }
}

#[test]
fn each_manifest_case_declares_the_reference_dependencies() {
    let cases = buildable_cases("write/lint", "manifests", |case: &ManifestCase| &case.steps);
    for case in &cases {
        let sandbox = Sandbox::new();
        for step in &case.steps {
            sandbox.apply(step);
        }
        let declared = read_declared_dependencies(&sandbox.path().join("repo"));
        let found = Declared {
            packages: declared.packages.into_iter().collect(),
            sources: declared.sources,
            warnings: declared.warnings,
            requires_python: declared.requires_python,
        };
        assert_eq!(found, case.expected, "{}", case.name);
    }
}

// A candidate set whose one edit names secret/x.py, with secret made unsearchable.
fn unsearchable(sandbox: &Sandbox) -> Option<(Services, Repo)> {
    let root = sandbox.repo("repo", &[("secret/x.py", "def f():\n    return 1\n")]);
    let edit = r#"{"edits": [{"path": "secret/x.py", "search": "1", "replace": "2"}]}"#;
    sandbox.put("cands/a.json", edit);
    if !sandbox.modes_bind() {
        return None;
    }
    let services = sandbox.services(sandbox.path());
    let repo = Repo::load(&services, &root).expect("the repository loads");
    sandbox.chmod("repo/secret", 0o000);
    Some((services, repo))
}

#[test]
fn a_target_whose_stat_fails_is_unreadable_not_absent() {
    let sandbox = Sandbox::new();
    let services = sandbox.services(sandbox.path());
    let long = sandbox.path().join("n".repeat(300));
    let error = load_lint_candidates(&services, &long).expect_err("the stat fails");
    let expected = format!(
        "cannot read <SANDBOX>/{}: File name too long",
        "n".repeat(300)
    );
    assert_eq!(
        refused(error, &sandbox),
        Expected::Error(("InputUnreadable".into(), expected))
    );
}

#[test]
fn an_edited_file_whose_stat_fails_is_a_read_that_failed() {
    let sandbox = Sandbox::new();
    let Some((services, repo)) = unsearchable(&sandbox) else {
        return;
    };
    let reads = super::tree::working_reads(&repo.root, &["secret/x.py"]);
    let candidates = load_lint_candidates(&services, &sandbox.path().join("cands"));
    let request = CandidatesRequest {
        candidates: candidates.expect("the candidate loads"),
        checks: CheckSelection::all(),
    };
    let mut miss = |_: &str, _: &str, _| Stored::Miss;
    let linted = linted(&services, &repo, &request, &mut miss);
    sandbox.chmod("repo/secret", 0o755);
    let unread = Some(Err("unreadable: Permission denied".to_owned()));
    assert_eq!(reads.get("secret/x.py"), Some(&unread));
    let report = compact(&linted.value.expect("a report"));
    let row = "\"message\": \"not checked: the text of secret/x.py before the change was not read \
               (unreadable: Permission denied), so no pair check compared this file.\"";
    assert!(report.contains(row), "{report}");
    assert!(!report.contains("no such file"), "{report}");
}

#[test]
fn a_manifest_whose_stat_fails_is_a_warning_not_an_absent_manifest() {
    let sandbox = Sandbox::new();
    sandbox.put("hidden/pyproject.toml", "[project]\ndependencies = []\n");
    sandbox.mkdir("repo");
    let link = [
        "symlink",
        "repo/pyproject.toml",
        "<SANDBOX>/hidden/pyproject.toml",
    ];
    sandbox.apply(&link.map(Value::from));
    if !sandbox.modes_bind() {
        return;
    }
    sandbox.chmod("hidden", 0o000);
    let declared = read_declared_dependencies(&sandbox.path().join("repo"));
    sandbox.chmod("hidden", 0o755);
    let warning = "pyproject.toml not read: unreadable: Permission denied";
    assert_eq!(declared.warnings, [warning]);
    assert!(declared.sources.is_empty());
}

#[test]
fn each_input_name_gives_the_reference_stem() {
    let cases: Vec<(String, String)> = io::testing::cases(MANIFEST_DIR, "write/lint", "stems");
    for (name, expected) in &cases {
        assert_eq!(&stem(Path::new(name)), expected, "{name:?}");
    }
}

#[test]
fn the_candidates_of_one_run_share_one_clone_index() {
    let sandbox = Sandbox::new();
    let total = "def total(items):\n    out = 0\n    for item in sorted(items, key=abs):\n        \
                 out += item * 2 - max(item, 0)\n    return round(out, 2) if out else 0\n";
    let root = sandbox.repo("repo", &[("app.py", total)]);
    let services = sandbox.services(sandbox.path());
    let repo = Repo::load(&services, &root).expect("the repository loads");
    let candidate = |index: usize| LintCandidateInput {
        id: format!("{index:02}"),
        parsed: patch::parse_blocks(&format!(
            "### lib{index}.py\n<<<<<<< SEARCH\n=======\n{total}>>>>>>> REPLACE\n"
        )),
        notes: Vec::new(),
    };
    let names = ["clone_delta".to_owned()];
    let request = CandidatesRequest {
        candidates: (0..3).map(candidate).collect(),
        checks: CheckSelection::from_names(&names).expect("a check"),
    };
    let looked_up: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let mut lookup = |path: &str, _: &str, _| {
        looked_up.borrow_mut().push(path.to_owned());
        Stored::Miss
    };

    let report = linted(&services, &repo, &request, &mut lookup).value;

    assert_eq!(report.expect("a report").candidates.len(), 3);
    assert_eq!(looked_up.into_inner(), ["app.py"]);
}

#[test]
fn a_run_without_a_pair_check_never_reads_a_stored_block() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "def add(a, b):\n    return a + b\n")]);
    let services = sandbox.services(sandbox.path());
    let repo = Repo::load(&services, &root).expect("the repository loads");
    let text = "### app.py\n<<<<<<< SEARCH\n    return a + b\n=======\n    return b + a\n>>>>>>> \
                REPLACE\n";
    let names = ["shadowing".to_owned()];
    let request = CandidatesRequest {
        candidates: vec![LintCandidateInput {
            id: "01".to_owned(),
            parsed: patch::parse_blocks(text),
            notes: Vec::new(),
        }],
        checks: CheckSelection::from_names(&names).expect("a check"),
    };
    let mut lookup = |path: &str, _: &str, _| panic!("{path}: no clone_delta, no index");

    let report = linted(&services, &repo, &request, &mut lookup).value;

    assert_eq!(report.expect("a report").candidates[0].findings, []);
}

fn golden_report() -> Value {
    let path: PathBuf = [
        MANIFEST_DIR,
        "../../tests/characterization/goldens/lint/repo_py.lint.json",
    ]
    .iter()
    .collect();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{error}"));
    let document: Value = serde_json::from_str(&text).expect("the golden is JSON");
    serde_json::json!({ "candidates": document["candidates"] })
}

// The golden's candidates over a sandbox copy of its repository, with its boundary config.
#[test]
fn the_lint_golden_report_comes_from_its_five_candidates() {
    let golden: Vec<Golden> = io::testing::cases(MANIFEST_DIR, "write/lint", "golden");
    let golden = &golden[0];
    let sandbox = Sandbox::new();
    for (name, text) in &golden.files {
        sandbox.put(&format!("repo/{name}"), text);
    }
    let services = sandbox.services(sandbox.path());
    let mut repo = Repo::load(&services, &sandbox.path().join("repo")).expect("a repository");
    repo.config
        .boundary_globs
        .clone_from(&golden.boundary_globs);
    let candidates = golden
        .candidates
        .iter()
        .map(|(id, text)| LintCandidateInput {
            id: id.clone(),
            parsed: crate::load_edits(text).expect("a golden candidate loads"),
            notes: Vec::new(),
        });
    let request = CandidatesRequest {
        candidates: candidates.collect(),
        checks: CheckSelection::all(),
    };
    let mut miss = |_: &str, _: &str, _| Stored::Miss;

    let found = linted(&services, &repo, &request, &mut miss);

    assert_eq!(found.diagnostics, Vec::<String>::new());
    let report = found.value.expect("the golden report");
    let data: Value = serde_json::from_str(&compact(&report)).expect("JSON");
    assert_eq!(data, golden_report());
    let checks: BTreeSet<&str> = report
        .candidates
        .iter()
        .flat_map(|candidate| candidate.findings.iter().map(|row| row.finding.check))
        .collect();
    let expected = [
        "arity",
        "clone_delta",
        "complexity_delta",
        "contract_drift",
        "dangling_callers",
        "dangling_references",
        "undeclared_imports",
    ];
    assert_eq!(checks, expected.into_iter().collect());
}
