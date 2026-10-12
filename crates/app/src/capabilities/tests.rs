use pycompat::{JsonForm, dumps};
use serde::Deserialize;
use serde_json::Value;

use super::{CAPS, CapValue, Setting, capabilities};
use crate::testing::{Sandbox, cases};
use crate::{Session, resolve};

#[derive(Deserialize)]
struct Case {
    name: String,
    config: Option<Value>,
    no_cache: bool,
    corrupt: Option<bool>,
    report: String,
}

// The two fields that differ from Python by design: the build version and the grammar source.
fn deviating(report: &str) -> (&'static str, &'static str) {
    let document: Value = serde_json::from_str(report).expect("a JSON report");
    let field = |name: &str| -> &'static str {
        let text = document[name].as_str().expect("a text field").to_owned();
        Box::leak(text.into_boxed_str())
    };
    (field("version"), field("grammar_cache"))
}

fn session(sandbox: &Sandbox, case: &Case) -> Session {
    let mut files = vec![("app.py", "x = 1\n".to_owned())];
    if let Some(config) = &case.config {
        files.push((".agentless-mcp.json", config.to_string()));
    }
    let listed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (*path, text.as_str()))
        .collect();
    let root = sandbox.repo("repo", &listed);
    let services = sandbox.services(sandbox.path());
    if case.corrupt == Some(true) {
        let cache_root = services.cache_root.as_ref().expect("a cache root");
        let database = store::database_path(&cache_root.path, root.to_str().expect("UTF-8"));
        let relative = database
            .strip_prefix(sandbox.path())
            .expect("a sandbox path");
        sandbox.put(relative.to_str().expect("UTF-8"), "not a database");
    }
    let resolved = resolve(&services, Some(&root)).expect("the repository resolves");
    Session::open(&services, resolved, case.no_cache)
        .expect("a session")
        .value
}

#[test]
fn each_report_equals_the_python_report_apart_from_the_build_fields() {
    let reports: Vec<Case> = cases("capabilities", "reports");
    for case in &reports {
        let sandbox = Sandbox::new();
        let session = session(&sandbox, case);
        let mut report = capabilities(&session).expect("a report");
        (report.version, report.grammar_cache) = deviating(&case.report);
        let key = session
            .cache
            .status
            .path
            .parent()
            .and_then(|parent| parent.file_name());
        let key = key.expect("a key directory").to_str().expect("UTF-8");

        let text = dumps(&report, JsonForm::Compact).expect("a report serializes");

        let shown = text
            .replace(key, "<KEY>")
            .replace(&sandbox.path().display().to_string(), "<SANDBOX>");
        assert_eq!(shown, case.report, "{}", case.name);
    }
}

#[test]
fn the_build_fields_name_this_build() {
    let sandbox = Sandbox::new();
    let case = Case {
        name: "plain".to_owned(),
        config: None,
        no_cache: false,
        corrupt: None,
        report: String::new(),
    };
    let session = session(&sandbox, &case);

    let report = capabilities(&session).expect("a report");

    assert_eq!(report.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(report.grammar_cache, "(linked into the binary)");
    assert!(report.languages.iter().all(|language| language.probe_ok));
}

#[test]
fn the_text_of_a_value_is_the_python_text() {
    let list = Setting::List(vec!["a".to_owned(), "b".to_owned()]);
    let shown: Vec<String> = [
        Setting::Bool(true),
        Setting::List(Vec::new()),
        list,
        Setting::Int(10),
        Setting::Text("auto".to_owned()),
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(shown, ["true", "none", "a, b", "10", "auto"]);
    assert_eq!(CapValue::Float(1.0).to_string(), "1.0");
    assert_eq!(CAPS.len(), 27);
}
