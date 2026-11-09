use std::path::{Path, PathBuf};

use app::trace::{Card, Change, Outcome, Revision, Section, Trace, TraceRequest};
use app::{EnvConfig, Reading, Repo, Services};
use git::Snapshot;
use io::testing::{Identity, Tree, hermetic_env, run_git};
use serde_json::Value;
use store::FactSource;

use crate::{Receipt, TraceAnswer, View, json, text};

const PLACEHOLDER: &str = "<REPO>";
const REQUEST: &str = ".request.json";
// The commit of every rig, so that its object ids and its last-change lines are fixed.
const IDENTITY: Identity = Identity {
    name: "render goldens",
    email: "render@example.invalid",
    date: "2026-02-01T00:00:00Z",
};
// A late git gives a card that says so; a retry reads the commit that the golden holds.
const LAST_CHANGE_TRIES: usize = 3;

fn characterization(parts: &[&str]) -> PathBuf {
    let base = [env!("CARGO_MANIFEST_DIR"), "../../tests/characterization"];
    base.iter().chain(parts).collect()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

// Each request beside a golden of `kind`, by the golden's stem, in name order.
pub(super) fn requests(kind: &str) -> Vec<(String, Value)> {
    let dir = characterization(&["goldens", kind]);
    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|error| panic!("{}: {error}", dir.display()));
    let mut found: Vec<(String, Value)> = entries
        .map(|entry| entry.unwrap_or_else(|error| panic!("{error}")).path())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?.to_owned();
            let stem = name.strip_suffix(REQUEST)?.to_owned();
            let request = serde_json::from_str(&read(&path));
            Some((
                stem,
                request.unwrap_or_else(|error| panic!("{name}: {error}")),
            ))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!found.is_empty(), "{}: no request", dir.display());
    found
}

// Every golden of `kind` has its request beside it.
pub(super) fn assert_each_golden_has_a_request(kind: &str) {
    let dir = characterization(&["goldens", kind]);
    let stems: Vec<String> = requests(kind).into_iter().map(|(stem, _)| stem).collect();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|error| panic!("{error}")) {
        let name = entry.unwrap_or_else(|error| panic!("{error}")).file_name();
        let name = name.to_string_lossy();
        if name.ends_with(REQUEST) {
            continue;
        }
        let stem = name.rsplit_once('.').map_or(&*name, |(stem, _)| stem);
        assert!(
            stems.iter().any(|known| known == stem),
            "{name}: no request"
        );
    }
}

// The golden of `stem`, or `None` when the request has no golden of that form.
pub(super) fn golden(kind: &str, stem: &str, suffix: &str) -> Option<String> {
    let path = characterization(&["goldens", kind, &format!("{stem}.{suffix}")]);
    path.is_file().then(|| read(&path))
}

// A copy of one fixture repository with the request's extra files, committed with the pinned
// identity, then the request's history, read with its git state.
pub(super) struct Rig {
    _tree: Tree,
    pub(super) services: Services,
    repo: Repo,
    snapshot: Snapshot,
    source: FactSource,
}

impl Rig {
    pub(super) fn new(request: &Value) -> Rig {
        let tree = Tree::new().unwrap_or_else(|error| panic!("a tree: {error}"));
        let fixture = request["fixture"].as_str().expect("a fixture name");
        let root = tree.mkdir("repo");
        copied(&characterization(&["fixtures", fixture]), &tree, "repo");
        extras(request, &tree);
        let env = hermetic_env(&tree.mkdir("home"));
        for args in [
            &["init", "--quiet", "-b", "main"][..],
            &["add", "-A"],
            &["commit", "--quiet", "-m", "fixture"],
        ] {
            drop(run_git(&root, args, &env, &IDENTITY));
        }
        history(request, &tree, |args| {
            drop(run_git(&root, args, &env, &IDENTITY));
        });
        let services = Services::new(&EnvConfig::new(env, root.clone()));
        let repo = Repo::load(&services, &root).unwrap_or_else(|error| panic!("{error}"));
        let snapshot = services.git.snapshot(&repo.root, Some(&repo.root));
        Rig {
            _tree: tree,
            services,
            repo,
            snapshot,
            source: FactSource::parsing(),
        }
    }

    pub(super) fn reading(&self) -> Reading<'_> {
        Reading {
            repo: &self.repo,
            snapshot: &self.snapshot,
            facts: &self.source,
            deadline: None,
        }
    }

    // The answer under its receipt, with the root as its placeholder.
    pub(super) fn text(&self, view: &impl View) -> String {
        self.text_at(view, None)
    }

    pub(super) fn json(&self, view: &impl View) -> String {
        self.json_at(view, None)
    }

    // As `text`, under the receipt of an answer that read `revision`.
    fn text_at(&self, view: &impl View, revision: Option<&Revision>) -> String {
        let receipt = Receipt::new(&self.repo, &self.snapshot, "none").at_revision(revision);
        text(&receipt, view).replace(&self.repo.root.display().to_string(), PLACEHOLDER)
    }

    fn json_at(&self, view: &impl View, revision: Option<&Revision>) -> String {
        let receipt = Receipt::new(&self.repo, &self.snapshot, "none").at_revision(revision);
        let document = json(&receipt, view).unwrap_or_else(|error| panic!("{error}"));
        document.replace(&self.repo.root.display().to_string(), PLACEHOLDER)
    }
}

fn copied(from: &Path, tree: &Tree, below: &str) {
    let entries = std::fs::read_dir(from).unwrap_or_else(|error| panic!("{error}"));
    for entry in entries {
        let path = entry.unwrap_or_else(|error| panic!("{error}")).path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("a name");
        let relative = format!("{below}/{name}");
        if path.is_dir() {
            drop(tree.mkdir(&relative));
            copied(&path, tree, &relative);
        } else {
            let copy = tree.copy_file(&path, &relative);
            copy.unwrap_or_else(|error| panic!("{relative}: {error}"));
        }
    }
}

// The request's `files` are written as given; each `sparse` file is that many zero bytes.
fn extras(request: &Value, tree: &Tree) {
    written(&request["files"], tree);
    for (path, size) in request["sparse"].as_object().into_iter().flatten() {
        let relative = format!("repo/{path}");
        let written = tree.write_file(&relative, "");
        written.unwrap_or_else(|error| panic!("{relative}: {error}"));
        let size = size.as_u64().expect("a size");
        let sized = tree.set_len(&relative, size);
        sized.unwrap_or_else(|error| panic!("{relative}: {error}"));
    }
}

fn written(files: &Value, tree: &Tree) {
    for (path, text) in files.as_object().into_iter().flatten() {
        let relative = format!("repo/{path}");
        if let Some((parent, _)) = relative.rsplit_once('/') {
            drop(tree.mkdir(parent));
        }
        let text = text.as_str().expect("a file's text");
        let written = tree.write_file(&relative, text);
        written.unwrap_or_else(|error| panic!("{relative}: {error}"));
    }
}

// The request's `tag` on the fixture commit, each of its `commits` with its files and message, and
// then its `working` files, which no commit holds.
fn history(request: &Value, tree: &Tree, git: impl Fn(&[&str])) {
    if let Some(tag) = request["tag"].as_str() {
        git(&["tag", tag]);
    }
    for commit in request["commits"].as_array().into_iter().flatten() {
        written(&commit["files"], tree);
        let message = commit["message"].as_str().expect("a commit message");
        git(&["add", "-A"]);
        git(&["commit", "--quiet", "-m", message]);
    }
    written(&request["working"], tree);
}

fn trace_request(arguments: &Value) -> TraceRequest {
    let targets = match &arguments["target"] {
        Value::String(target) => vec![target.clone()],
        Value::Array(targets) => (targets.iter())
            .map(|target| target.as_str().expect("a target").to_owned())
            .collect(),
        other => panic!("target: {other}"),
    };
    let more = arguments["more"]
        .as_str()
        .map(|name| Section::from_name(name).unwrap_or_else(|| panic!("more: {name}")));
    TraceRequest { targets, more }
}

fn late(trace: &Trace) -> bool {
    (trace.cards.iter()).any(|traced| match &traced.outcome {
        Outcome::Card(Card::Symbol(card)) => matches!(card.change, Some(Change::NotRead { .. })),
        Outcome::Card(Card::File(card)) => matches!(card.change, Some(Change::NotRead { .. })),
        _ => false,
    })
}

fn traced(rig: &Rig, asked: (&TraceRequest, Option<&str>)) -> Trace {
    for _ in 1..LAST_CHANGE_TRIES {
        let trace = traced_once(rig, asked);
        if !late(&trace) {
            return trace;
        }
    }
    traced_once(rig, asked)
}

fn traced_once(rig: &Rig, (request, rev): (&TraceRequest, Option<&str>)) -> Trace {
    let noted = app::trace::trace_at(&rig.services, &rig.reading(), request, rev);
    assert!(noted.diagnostics.is_empty(), "{:?}", noted.diagnostics);
    noted.value.unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn the_trace_goldens_come_out_byte_for_byte() {
    let mut mismatches = Vec::new();
    for (stem, request) in requests("trace") {
        let rig = Rig::new(&request);
        let asked = trace_request(&request["arguments"]);
        let rev = request["arguments"]["rev"].as_str();
        let trace = traced(&rig, (&asked, rev));
        let answer = TraceAnswer {
            request: &asked,
            trace: &trace,
        };
        let revision = trace.revision.as_ref();
        mismatches.extend(compared(
            "trace",
            &stem,
            &rig.text_at(&answer, revision),
            &rig.json_at(&answer, revision),
        ));
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

// Each form that differs from its golden, with what it renders. Every request has a text golden;
// a JSON golden is kept for one request of each card kind and answer form.
pub(super) fn compared(kind: &str, stem: &str, text: &str, json: &str) -> Vec<String> {
    let mut mismatches = Vec::new();
    if golden(kind, stem, "txt").as_deref() != Some(text) {
        mismatches.push(format!("{stem}.txt renders:\n{text}"));
    }
    let kept = golden(kind, stem, "json");
    if kept.as_deref().is_some_and(|kept| kept != json) {
        mismatches.push(format!("{stem}.json renders:\n{json}"));
    }
    mismatches
}

#[test]
fn each_trace_golden_has_its_request_beside_it() {
    assert_each_golden_has_a_request("trace");
}
