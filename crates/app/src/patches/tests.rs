use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use json::{JsonForm, to_text};
use patch::{Edit, ParseResult};
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use super::{
    ApplyMode, INTERRUPTED, Target, applied_at, apply_patch, canonical, check_patch, load_edits,
    normalize_patch,
};
use crate::testing::{MANIFEST_DIR, Sandbox, buildable_cases};
use crate::{Error, Noted, Repo};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Loaded {
    Parsed(String),
    Edits(usize),
    Error(String),
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Expected {
    Report(String),
    Error((String, String)),
}

#[derive(Debug, Deserialize)]
struct Operation {
    name: String,
    steps: Vec<Vec<Value>>,
    op: String,
    edits: String,
    mode: Option<String>,
    root: Option<String>,
    expected: Expected,
    diagnostics: Vec<String>,
    tree: BTreeMap<String, String>,
}

fn loaded(text: &str) -> Loaded {
    match load_edits(text) {
        Ok(parsed) => Loaded::Parsed(compact(&parsed)),
        Err(error) => Loaded::Error(error.to_string()),
    }
}

fn compact(value: &impl Serialize) -> String {
    to_text(value, JsonForm::Compact).expect("a report serializes")
}

#[test]
fn each_edits_text_loads_to_the_reference_document_or_refusal() {
    let cases: Vec<(String, Loaded)> = io::testing::cases(MANIFEST_DIR, "write/edits", "texts");
    for (text, expected) in &cases {
        assert_eq!(&loaded(text), expected, "{text:?}");
    }
}

// The same texts the generator builds for the bounds.
fn recipe_text(form: &str, count: usize, rows: usize) -> String {
    let old = vec!["x"; rows].join("\n");
    let new = vec!["y"; rows].join("\n");
    let paths = (0..count).map(|index| format!("pkg/mod_{index:04}.py"));
    if form == "blocks" {
        let blocks = paths.map(|path| {
            format!("### {path}\n<<<<<<< SEARCH\n{old}\n=======\n{new}\n>>>>>>> REPLACE\n\n")
        });
        return blocks.collect();
    }
    let edits: Vec<Value> = paths
        .enumerate()
        .map(|(index, path)| {
            serde_json::json!({"index": index, "path": path, "search": old, "replace": new})
        })
        .collect();
    compact(&serde_json::json!({ "edits": edits }))
}

#[test]
fn each_bounded_request_loads_or_is_refused_as_the_reference_says() {
    let cases: Vec<(String, usize, usize, Loaded)> =
        io::testing::cases(MANIFEST_DIR, "write/edits", "recipes");
    for (form, count, rows, expected) in &cases {
        let found = match load_edits(&recipe_text(form, *count, *rows)) {
            Ok(parsed) => Loaded::Edits(parsed.edits.len()),
            Err(error) => Loaded::Error(error.to_string()),
        };
        assert_eq!(&found, expected, "{form} {count} {rows}");
    }
}

fn refusal(text: &str) -> String {
    load_edits(text)
        .expect_err("a refused document")
        .to_string()
}

// One edits.json entry with `index` as the JSON text `index`.
fn indexed(index: &str) -> String {
    format!(r#"{{"edits": [{{"path": "a.py", "search": "x", "replace": "y", "index": {index}}}]}}"#)
}

#[test]
fn a_bool_index_is_refused_as_a_non_integer() {
    assert_eq!(
        refusal(&indexed("true")),
        "edit 0 has a non-integer 'index'"
    );
}

#[test]
fn an_index_past_64_bits_is_refused() {
    let message = "edit 0 has an 'index' outside the 64-bit integer range";
    assert_eq!(refusal(&indexed("9223372036854775808")), message);
    assert_eq!(refusal(&indexed("-9223372036854775809")), message);
    let parsed = load_edits(&indexed("-9223372036854775808")).expect("the least i64 is an index");
    assert_eq!(parsed.edits[0].index, i64::MIN);
}

#[test]
fn a_lone_surrogate_in_a_text_field_is_refused() {
    for field in ["path", "search", "replace"] {
        let mut fields = serde_json::Map::new();
        for name in ["path", "search", "replace"] {
            fields.insert(name.to_owned(), Value::from("x"));
        }
        let text = compact(&serde_json::json!({ "edits": [fields] })).replace(
            &format!(r#""{field}": "x""#),
            &format!(r#""{field}": "\ud800""#),
        );
        let expected = format!("edit 0 has a lone surrogate in '{field}'");
        assert_eq!(refusal(&text), expected);
    }
}

#[test]
fn an_integer_over_the_digit_limit_is_not_valid_json() {
    let message = refusal(&indexed(&"1".repeat(4301)));
    assert!(
        message.starts_with("edits document is not valid JSON: Exceeds the limit (4300 digits)"),
        "{message}"
    );
}

// The error kinds by the reference's class names.
fn kind_name(error: &kernel::Error) -> &'static str {
    match error {
        kernel::Error::OperationFailed(_) => "OperationFailed",
        kernel::Error::SecurityRefusal(_) => "SecurityRefusal",
        kernel::Error::RepoResolution(_) => "RepoResolutionError",
        kernel::Error::InputUnreadable(_) => "InputUnreadable",
        other => panic!("an unexpected kind {other:?}"),
    }
}

fn shown<T: Serialize>(sandbox: &Sandbox, found: Result<T, Error>) -> Expected {
    let place = sandbox.path().display().to_string();
    match found {
        Ok(report) => Expected::Report(compact(&report).replace(&place, "<SANDBOX>")),
        Err(Error::Kind(kind)) => {
            let message = kind.to_string().replace(&place, "<SANDBOX>");
            Expected::Error((kind_name(&kind).to_owned(), message))
        }
        Err(other) => panic!("a defect: {other:?}"),
    }
}

fn edits_of(text: &str) -> Vec<Edit> {
    let parsed: ParseResult = load_edits(text).expect("the case's edits load");
    parsed.edits
}

// Runs the call of `case` and returns its result and warnings.
fn run(sandbox: &Sandbox, case: &Operation) -> (Expected, Vec<String>) {
    let services = sandbox.services(sandbox.path());
    let root = sandbox.path().join(case.root.as_deref().unwrap_or("repo"));
    let repo = Repo::load(&services, &root).expect("the repository loads");
    let edits = edits_of(&case.edits);
    match case.op.as_str() {
        "check" => (shown(sandbox, check_patch(&repo, &edits)), Vec::new()),
        "normalize" => (shown(sandbox, normalize_patch(&repo, &edits)), Vec::new()),
        _ => {
            let enclosing = services.git.root(&repo.root);
            let snapshot = services.git.snapshot(&repo.root, enclosing.as_deref());
            let mode = match case.mode.as_deref() {
                Some("in_place") => ApplyMode::InPlace,
                _ => ApplyMode::Worktree,
            };
            let Noted { value, diagnostics } =
                apply_patch(&services, &repo, &snapshot, &edits, mode);
            (shown(sandbox, value), diagnostics)
        }
    }
}

// Each file below `root` outside .git, as latin-1 text. Directory symlinks are not followed.
fn tree(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.map(|entry| entry.expect("a directory entry")) {
            let path = entry.path();
            let kind = entry.file_type().expect("a file type");
            if kind.is_dir() && entry.file_name() != ".git" {
                pending.push(path);
            } else if path.is_file() {
                let relative = path.strip_prefix(root).expect("a path below the root");
                let bytes = fs::read(&path).expect("a readable file");
                let text = bytes.iter().copied().map(char::from).collect();
                files.insert(relative.to_string_lossy().into_owned(), text);
            }
        }
    }
    files
}

// The chmod steps run last, and their modes are undone before the sandbox is removed.
fn operated(case: &Operation) {
    let sandbox = Sandbox::new();
    let modes: Vec<&Vec<Value>> = case
        .steps
        .iter()
        .filter(|step| step[0] == "chmod")
        .collect();
    for step in case.steps.iter().filter(|step| step[0] != "chmod") {
        sandbox.apply(step);
    }
    for step in &modes {
        let mode = step[2].as_u64().expect("a mode");
        sandbox.chmod(
            step[1].as_str().expect("a place"),
            u32::try_from(mode).expect("a mode"),
        );
    }
    let (found, diagnostics) = run(&sandbox, case);
    for step in &modes {
        sandbox.chmod(step[1].as_str().expect("a place"), 0o755);
    }
    assert_eq!(found, case.expected, "{}", case.name);
    assert_eq!(diagnostics, case.diagnostics, "{} warnings", case.name);
    if case.op == "apply" {
        assert_eq!(
            tree(&sandbox.path().join("repo")),
            case.tree,
            "{} tree",
            case.name
        );
    }
}

fn operations(section: &str) -> Vec<Operation> {
    buildable_cases("write/patch", section, |case: &Operation| &case.steps)
}

#[test]
fn each_check_case_gives_the_reference_report() {
    operations("check").iter().for_each(operated);
}

#[test]
fn each_normalize_case_gives_the_reference_report() {
    operations("normalize").iter().for_each(operated);
}

#[test]
fn each_apply_case_gives_the_reference_report_and_tree() {
    let sandbox = Sandbox::new();
    let modes_bind = sandbox.modes_bind();
    let cases = operations("apply");
    let kept = cases
        .iter()
        .filter(|case| modes_bind || !case.steps.iter().any(|step| step[0] == "chmod"));
    kept.for_each(operated);
}

#[test]
fn an_interrupt_during_the_write_keeps_the_write_and_takes_no_diff() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "x = 1\n")]);
    let services = sandbox.services(sandbox.path());
    let edits = edits_of("### app.py\n<<<<<<< SEARCH\nx = 1\n=======\nx = 2\n>>>>>>> REPLACE\n");

    let applied = applied_at(&services.git, &Target::working_tree(&root), &edits, || true);

    let Err(Error::Kind(kernel::Error::CallStopped(message))) = applied.value else {
        panic!("a stopped call: {:?}", applied.value);
    };
    assert_eq!(message, INTERRUPTED);
    assert_eq!(
        fs::read_to_string(root.join("app.py")).expect("app.py"),
        "x = 2\n"
    );
}

#[test]
fn a_patch_that_did_not_apply_is_not_stopped_by_an_interrupt() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "x = 1\n")]);
    let services = sandbox.services(sandbox.path());
    let edits = edits_of("### app.py\n<<<<<<< SEARCH\nnowhere\n=======\nx = 2\n>>>>>>> REPLACE\n");

    let applied = applied_at(&services.git, &Target::working_tree(&root), &edits, || true);

    let report = applied.value.expect("a report");
    assert!(!report.ok());
    assert_eq!(report.diff, "");
}

#[test]
fn canonical_paths_are_relative_and_the_root_is_a_dot() {
    let sandbox = Sandbox::new();
    let root = sandbox.repo("repo", &[("app.py", "x = 1\n")]);
    let edit = |path: &str| Edit {
        index: 0,
        path: path.to_owned(),
        search: String::new(),
        replace: String::new(),
    };
    let absolute = root.join("app.py").display().to_string();
    let edits = [
        edit("./app.py"),
        edit(""),
        edit("."),
        edit(&absolute),
        edit("a/../b.py"),
    ];

    let found = canonical(&root, &edits).expect("every path is inside");

    let paths: Vec<&str> = found.iter().map(|edit| edit.path.as_str()).collect();
    assert_eq!(paths, ["app.py", ".", ".", "app.py", "b.py"]);
}
