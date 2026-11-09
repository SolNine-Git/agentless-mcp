use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use facts::line_prefix;
use git::Snapshot;
use kernel::testing::PinnedClock;
use store::FactSource;

use crate::testing::Sandbox;
use crate::trace::{
    Card, Change, KeptGraphs, Miss, Outcome, Revision, Section, SymbolTrace, Trace, TraceRequest,
    trace_at, trace_kept_at,
};
use crate::{Error, IndexRequest, Reading, Repo, Services, Session, index};

// The base commit holds `load` at lines 1-2; HEAD moved it to lines 4-5, and the working tree
// adds a file that no commit holds.
const BASE: [(&str, &str); 3] = [
    ("pkg/__init__.py", ""),
    (
        "pkg/svc.py",
        "def load(path):\n    return path\n\n\ndef helper():\n    return load(1)\n",
    ),
    (
        "pkg/use.py",
        "from pkg.svc import load\n\n\ndef run():\n    return load(2)\n",
    ),
];
const MOVED: &str = "import os\n\n\ndef load(path):\n    return os.fspath(path)\n\n\n\
                     def helper():\n    return load(1)\n";
const LOAD: &str = "py:pkg/svc.py::load";

struct Fixture {
    sandbox: Sandbox,
    root: PathBuf,
    services: Services,
    repo: Repo,
    snapshot: Snapshot,
}

impl Fixture {
    fn new() -> Fixture {
        let sandbox = Sandbox::new();
        let root = sandbox.repo("repo", &BASE);
        sandbox.git(&root, &["tag", "base"]);
        sandbox.put("repo/pkg/svc.py", MOVED);
        sandbox.git(&root, &["commit", "--quiet", "-am", "move load"]);
        sandbox.put("repo/pkg/new.py", "def fresh():\n    return 3\n");
        let services = sandbox.services(&root);
        let repo = Repo::load(&services, &root).unwrap_or_else(|error| panic!("{error}"));
        let snapshot = services.git.snapshot(&root, Some(&root));
        Fixture {
            sandbox,
            root,
            services,
            repo,
            snapshot,
        }
    }

    fn traced(&self, targets: &[&str], more: Option<Section>, rev: &str) -> Result<Trace, Error> {
        let source = FactSource::parsing();
        let reading = self.reading(&source);
        trace_at(&self.services, &reading, &request(targets, more), Some(rev)).value
    }

    fn reading<'a>(&'a self, source: &'a FactSource) -> Reading<'a> {
        Reading {
            repo: &self.repo,
            snapshot: &self.snapshot,
            facts: source,
            deadline: None,
        }
    }

    fn card(&self, target: &str, more: Option<Section>) -> Card {
        let mut traced = self
            .traced(&[target], more, "base")
            .expect("a trace at base");
        match traced.cards.remove(0).outcome {
            Outcome::Card(card) => card,
            other => panic!("{target}: a card, got {other:?}"),
        }
    }

    fn commit(&self, revision: &str) -> String {
        let printed = self.sandbox.git(&self.root, &["rev-parse", revision]);
        String::from_utf8(printed)
            .expect("a hex id")
            .trim()
            .to_owned()
    }

    fn shown(&self, revision: &str, path: &str) -> String {
        let object = format!("{revision}:{path}");
        let printed = self.sandbox.git(&self.root, &["show", &object]);
        String::from_utf8(printed).expect("a UTF-8 file")
    }
}

fn request(targets: &[&str], more: Option<Section>) -> TraceRequest {
    TraceRequest {
        targets: targets.iter().map(|target| (*target).to_owned()).collect(),
        more,
    }
}

fn symbol(card: Card) -> SymbolTrace {
    match card {
        Card::Symbol(traced) => *traced,
        other => panic!("a symbol card, got {other:?}"),
    }
}

// The lines `start..=end` of `text`, each behind its line number, as a body shows them.
fn numbered(text: &str, (start, end): (u32, u32)) -> String {
    let lines = text.split('\n').skip(start as usize - 1);
    let rows = (start..=end).zip(lines);
    let rows: Vec<String> = rows
        .map(|(at, line)| format!("{}{line}", line_prefix(at)))
        .collect();
    rows.join("\n")
}

fn refusal(traced: Result<Trace, Error>) -> String {
    match traced {
        Err(error) => error.to_string(),
        Ok(trace) => panic!("a refusal, got {trace:?}"),
    }
}

#[test]
fn a_body_at_a_revision_is_the_commits_text_cut_to_the_span() {
    let fixture = Fixture::new();

    let traced = symbol(fixture.card(LOAD, Some(Section::Body)));

    let body = traced.body.expect("a body").expect("a read body");
    assert_eq!((body.start_line, body.end_line), (1, 2));
    assert_eq!(
        body.body,
        numbered(&fixture.shown("base", "pkg/svc.py"), (1, 2))
    );
    assert_eq!(traced.card.start_line, 1);
}

#[test]
fn a_line_of_the_commits_file_names_the_symbol_that_spans_it() {
    let fixture = Fixture::new();

    let traced = symbol(fixture.card("pkg/svc.py:6", Some(Section::Body)));

    let body = traced.body.expect("a body").expect("a read body");
    assert_eq!(traced.card.stable_id, "py:pkg/svc.py::helper");
    assert_eq!(
        body.body,
        numbered(&fixture.shown("base", "pkg/svc.py"), (5, 6))
    );
}

#[test]
fn the_one_blob_path_and_the_scan_of_the_commit_give_the_same_card() {
    let fixture = Fixture::new();
    for more in [Section::Body, Section::History] {
        let alone = fixture.card(LOAD, Some(more));
        let scanned = fixture.traced(&[LOAD, "helper"], Some(more), "base");
        let mut scanned = scanned.expect("a trace at base");
        let Outcome::Card(scanned) = scanned.cards.remove(0).outcome else {
            panic!("a card");
        };
        assert_eq!(scanned, alone, "{more:?}");
    }
}

#[test]
fn a_history_at_a_revision_lists_the_commits_in_that_commits_history() {
    let fixture = Fixture::new();

    let traced = symbol(fixture.card(LOAD, Some(Section::History)));

    let history = traced.history.expect("a history").expect("a read history");
    let shas: Vec<&str> = history
        .entries
        .iter()
        .map(|entry| entry.sha.as_str())
        .collect();
    assert_eq!(shas, [fixture.commit("base")]);
    let lines = (history.committed_start_line, history.committed_end_line);
    assert_eq!(lines, (1, 2));
    assert_eq!(history.dirty, Some(false));
}

#[test]
fn a_symbol_card_at_a_revision_has_the_commits_lines_callers_and_last_change() {
    let fixture = Fixture::new();

    let traced = symbol(fixture.card("load", None));

    assert_eq!((traced.card.start_line, traced.card.end_line), (1, 2));
    let callers = traced.callers.expect("callers");
    let files: Vec<&str> = callers.rows.iter().map(|row| row.path.as_str()).collect();
    assert_eq!(files, ["pkg/svc.py", "pkg/use.py"]);
    let Some(Change::Commit(change)) = traced.change else {
        panic!("a last change, got {:?}", traced.change);
    };
    assert_eq!(change.sha, fixture.commit("base"));
}

#[test]
fn a_file_card_at_a_revision_counts_the_commits_lines_and_symbols() {
    let fixture = Fixture::new();

    let Card::File(file) = fixture.card("pkg/svc.py", None) else {
        panic!("a file card");
    };

    assert_eq!(file.lines, 6);
    let declares = file.declares.expect("declarations");
    let lines: Vec<u32> = declares.rows.iter().map(|row| row.start_line).collect();
    assert_eq!(lines, [1, 5]);
    let Some(Change::Commit(change)) = file.change else {
        panic!("a last change, got {:?}", file.change);
    };
    assert_eq!(change.sha, fixture.commit("base"));
}

#[test]
fn a_path_that_the_commit_lacks_and_the_working_tree_holds_is_named_so() {
    let fixture = Fixture::new();
    for target in ["pkg/new.py", "pkg/new.py:1"] {
        let Card::Miss(miss) = fixture.card(target, None) else {
            panic!("{target}: a miss");
        };
        let path = "pkg/new.py".to_owned();
        assert_eq!(miss, Miss::AbsentAtRevision { path }, "{target}");
    }
}

#[test]
fn the_answer_names_the_revision_as_given_and_its_commit() {
    let fixture = Fixture::new();
    for more in [None, Some(Section::Body)] {
        let traced = fixture
            .traced(&[LOAD], more, "base")
            .expect("a trace at base");
        let revision = Revision {
            name: "base".to_owned(),
            commit: fixture.commit("base"),
        };
        assert_eq!(traced.revision, Some(revision));
    }
}

#[test]
fn an_unknown_rev_is_refused_with_gits_note() {
    let fixture = Fixture::new();

    let refused = refusal(fixture.traced(&[LOAD], None, "nosuch"));

    let head = "rev nosuch names no commit: git rev-parse exited 128: ";
    assert!(refused.starts_with(head), "{refused}");
    assert!(refused.ends_with("such as rev=main."), "{refused}");
}

#[test]
fn a_rev_that_holds_a_colon_or_starts_with_a_dash_never_reaches_git() {
    let fixture = Fixture::new();
    for rev in ["base:pkg", "-n1", "--all"] {
        let refused = refusal(fixture.traced(&[LOAD], None, rev));
        let text =
            format!("rev {rev} is not a revision name: it holds \":\" or starts with \"-\".");
        assert!(refused.ends_with(&text), "{refused}");
    }
}

#[test]
fn an_unknown_rev_in_a_shallow_clone_says_that_the_clone_is_shallow() {
    let fixture = Fixture::new();
    let url = format!("file://{}", fixture.root.display());
    let clone = fixture.sandbox.path().join("clone");
    let target = clone.to_str().expect("a UTF-8 path");
    let top = fixture.sandbox.path();
    fixture
        .sandbox
        .git(top, &["clone", "--quiet", "--depth", "1", &url, target]);
    let services = fixture.sandbox.services(&clone);
    let repo = Repo::load(&services, &clone).unwrap_or_else(|error| panic!("{error}"));
    let snapshot = services.git.snapshot(&clone, Some(&clone));
    let source = FactSource::parsing();
    let reading = Reading {
        repo: &repo,
        snapshot: &snapshot,
        facts: &source,
        deadline: None,
    };

    let traced = trace_at(&services, &reading, &request(&[LOAD], None), Some("HEAD~1"));

    let refused = refusal(traced.value);
    assert!(
        refused.contains("rev HEAD~1 names no commit: "),
        "{refused}"
    );
    let shallow = "The clone is shallow: the commit can be older than its depth (git fetch \
                   --unshallow).";
    assert!(refused.ends_with(shallow), "{refused}");
}

#[test]
fn a_call_at_a_revision_writes_nothing_to_the_tag_cache() {
    let fixture = Fixture::new();
    let request = IndexRequest {
        force: false,
        max_cache_bytes: None,
    };
    let indexed = index(
        &fixture.services,
        &fixture.repo,
        &fixture.snapshot,
        &request,
    );
    let database = indexed.value.expect("the index run succeeds").database;
    let before = std::fs::read(&database).expect("the database");
    let repo = Repo::load(&fixture.services, &fixture.root).expect("the repository loads");
    let session = Session::open_facts(&fixture.services, repo, false)
        .session
        .value;
    let session = session.expect("a session over the database");
    let reading = Reading::of(&session);

    for more in [None, Some(Section::Body)] {
        let targets = request_of(more);
        let traced = trace_at(&fixture.services, &reading, &targets, Some("base"));
        traced.value.expect("a trace at base");
    }

    assert!(std::fs::read(&database).expect("the database") == before);
}

fn request_of(more: Option<Section>) -> TraceRequest {
    request(&[LOAD, "pkg/svc.py", "pkg"], more)
}

#[test]
fn two_calls_at_one_revision_share_one_kept_graph_that_never_updates() {
    let fixture = Fixture::new();
    let clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
    let graphs = KeptGraphs::new(clock);
    let source = FactSource::parsing();
    let reading = fixture.reading(&source);
    let call = |rev: Option<&str>| {
        let asked = request(&["load"], None);
        let traced = trace_kept_at(&fixture.services, &reading, &asked, (rev, &graphs));
        let mut traced = traced.value.expect("a trace");
        match traced.cards.remove(0).outcome {
            Outcome::Card(card) => symbol(card),
            other => panic!("a card, got {other:?}"),
        }
    };

    let first = call(Some("base"));
    let second = call(Some("base"));
    let builds = graphs.builds();
    let working = call(None);

    assert_eq!(first, second);
    assert_eq!((builds.graphs, builds.updates, builds.rebuilds), (1, 0, 0));
    assert_eq!(working.card.start_line, 4);
    assert_eq!(graphs.builds().graphs, 2);
}
