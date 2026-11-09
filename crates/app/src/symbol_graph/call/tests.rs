use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use kernel::testing::PinnedClock;

use super::{Builds, Gate, KeptGraphs, State};
use crate::endpoints::scanned::{LINKED, Scanned};
use crate::scan::KeptFacts;
use crate::symbol_graph::retention::{RETAINED, UPDATED, Updates};
use crate::trace::{CardKind, Outcome, TraceRequest, trace_kept};

fn pinned() -> (Arc<PinnedClock>, KeptGraphs) {
    let clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
    let graphs = KeptGraphs::new(Arc::clone(&clock) as _);
    (clock, graphs)
}

fn roots(names: &[String]) -> Vec<PathBuf> {
    names.iter().map(PathBuf::from).collect()
}

fn working(root: &Path) -> State<'_> {
    State { root, commit: None }
}

// The root of each kept state, in key order.
fn kept_roots(graphs: &KeptGraphs) -> Vec<PathBuf> {
    let states = graphs.kept_states().into_iter();
    states.map(|(root, _)| root).collect()
}

#[test]
fn a_server_keeps_the_state_of_the_roots_it_called_last() {
    let (_, graphs) = pinned();
    let names: Vec<String> = (0..=RETAINED.roots).map(|at| format!("r{at}")).collect();
    let facts: Vec<Arc<KeptFacts>> = names.iter().map(|_| Arc::default()).collect();
    for (name, facts) in names.iter().zip(&facts) {
        graphs.keep(working(Path::new(name)), Arc::clone(facts));
    }
    assert_eq!(kept_roots(&graphs), roots(&names[1..]));
    let oldest = &names[1];
    assert!(Arc::ptr_eq(
        &graphs.facts(working(Path::new(oldest))),
        &facts[1]
    ));

    graphs.keep(working(Path::new(&names[0])), Arc::clone(&facts[0]));

    let mut kept = vec![names[0].clone(), oldest.clone()];
    kept.extend(names[3..].iter().cloned());
    assert_eq!(kept_roots(&graphs), roots(&kept));
    assert!(Arc::ptr_eq(
        &graphs.facts(working(Path::new(oldest))),
        &facts[1]
    ));
}

#[test]
fn each_revision_of_a_root_counts_as_one_more_kept_state() {
    let (clock, graphs) = pinned();
    let root = Path::new("r");
    let commits: Vec<String> = (0..RETAINED.roots).map(|at| format!("c{at}")).collect();
    graphs.keep(working(root), Arc::default());
    for commit in &commits {
        let commit = Some(commit.as_str());
        graphs.keep(State { root, commit }, Arc::default());
    }

    let at = |commit: &String| (root.to_path_buf(), Some(commit.clone()));
    assert_eq!(
        graphs.kept_states(),
        commits.iter().map(at).collect::<Vec<_>>()
    );

    clock.advance(RETAINED.idle);
    graphs.kept.sweep();
    assert!(graphs.kept_states().is_empty());
}

#[test]
fn a_server_drops_the_state_of_a_root_after_the_idle_time_with_no_call() {
    let (clock, graphs) = pinned();
    let (early, late) = (Path::new("early"), Path::new("late"));
    let facts: Arc<KeptFacts> = Arc::default();
    graphs.keep(working(early), Arc::clone(&facts));
    let short = RETAINED.idle.checked_sub(Duration::from_secs(1));
    clock.advance(short.expect("the idle time is over a second"));
    graphs.keep(working(late), Arc::default());
    graphs.kept.sweep();
    assert_eq!(kept_roots(&graphs), [early, late]);
    assert!(Arc::ptr_eq(&graphs.facts(working(early)), &facts));

    clock.advance(RETAINED.idle);
    graphs.kept.sweep();
    assert!(kept_roots(&graphs).is_empty());

    graphs.keep(working(early), Arc::clone(&facts));
    clock.advance(RETAINED.idle);
    let after = graphs.facts(working(early));
    assert!(
        !Arc::ptr_eq(&after, &facts),
        "a call after the idle time reads no kept facts"
    );
    assert_eq!(kept_roots(&graphs), [early]);
}

// A server's state whose prebuilds wait for `open`, and send on `done` when they finish. A scan
// that waits for a prebuild sends on `waiting`.
struct Held {
    graphs: KeptGraphs,
    open: Sender<()>,
    done: Receiver<()>,
    waiting: Receiver<()>,
}

impl Held {
    fn new() -> Held {
        let (open, opened) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let (waits, waiting) = mpsc::channel();
        let gate = Gate {
            open: Mutex::new(opened),
            done: Mutex::new(finished),
            waiting: Mutex::new(waits),
        };
        let clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
        let graphs = KeptGraphs::gated(clock, gate);
        Held {
            graphs,
            open,
            done,
            waiting,
        }
    }

    fn kind(&self, scanned: &Scanned, target: &str) -> CardKind {
        kind(&self.graphs, scanned, target)
    }

    // Lets one prebuild run, and waits until it is done.
    fn prebuilt(&self) {
        self.open
            .send(())
            .expect("a graph thread waits at the gate");
        self.done.recv().expect("the prebuild says when it is done");
    }

    // How many times the graph threads built the name index, scopes, resolver and graph, then the
    // aliases, ambiguous degrees, import graph and cycles.
    fn parts(&self) -> [usize; 8] {
        let counts = &self.graphs.builds;
        [
            &counts.indexes,
            &counts.scopes,
            &counts.resolvers,
            &counts.graphs,
            &counts.aliases,
            &counts.degrees,
            &counts.imports,
            &counts.cycles,
        ]
        .map(|count| count.load(Ordering::Relaxed))
    }
}

// The kind of the card of `target`, traced over the kept graphs.
fn kind(graphs: &KeptGraphs, scanned: &Scanned, target: &str) -> CardKind {
    let request = TraceRequest {
        targets: vec![target.to_owned()],
        more: None,
    };
    let traced = trace_kept(&scanned.services, &scanned.reading(), &request, graphs);
    let mut trace = traced.value.unwrap_or_else(|error| panic!("{error}"));
    match trace.cards.remove(0).outcome {
        Outcome::Card(card) => card.kind(),
        other => panic!("{target}: a card, got {other:?}"),
    }
}

#[test]
fn a_call_that_needs_no_graph_answers_while_its_prebuild_is_held_back() {
    let scanned = Scanned::new(&LINKED);
    let held = Held::new();
    assert_eq!(held.kind(&scanned, "pkg/report.py"), CardKind::File);
    assert_eq!(held.parts(), [1, 1, 0, 0, 0, 0, 1, 1]);
    held.prebuilt();
    assert_eq!(held.parts(), [1; 8]);
}

#[test]
fn a_symbol_call_after_the_prebuild_builds_nothing() {
    let scanned = Scanned::new(&LINKED);
    let held = Held::new();
    assert_eq!(held.kind(&scanned, "pkg/"), CardKind::Folder);
    held.prebuilt();
    assert_eq!(held.parts(), [1; 8]);
    assert_eq!(held.kind(&scanned, "render"), CardKind::Symbol);
    assert_eq!(held.kind(&scanned, "pkg/report.py"), CardKind::File);
    assert_eq!(held.parts(), [1; 8]);
}

#[test]
fn a_changed_scan_starts_one_build() {
    let scanned = Scanned::new(&LINKED);
    let held = Held::new();
    assert_eq!(held.kind(&scanned, "pkg/report.py"), CardKind::File);
    held.prebuilt();
    let edited = scanned.reading().repo.root.join("pkg/money.py");
    let text = "def format_money(amount):\n    return amount\n\n\ndef added():\n    return 2\n";
    io::write_file(&edited, text.as_bytes())
        .unwrap_or_else(|error| panic!("{}: {error}", edited.display()));
    assert_eq!(held.kind(&scanned, "pkg/report.py"), CardKind::File);
    assert_eq!(held.parts(), [2, 2, 1, 1, 1, 1, 2, 2]);
    held.prebuilt();
    assert_eq!(held.parts(), [2; 8]);
    assert_eq!(held.kind(&scanned, "added"), CardKind::Symbol);
    assert_eq!(held.kind(&scanned, "pkg/report.py"), CardKind::File);
    assert_eq!(held.parts(), [2; 8]);
}

#[test]
fn an_edit_that_changes_no_fact_keeps_the_graph() {
    let scanned = Scanned::new(&LINKED);
    let held = Held::new();
    assert_eq!(held.kind(&scanned, "pkg/report.py"), CardKind::File);
    held.prebuilt();
    let edited = scanned.reading().repo.root.join("pkg/money.py");
    let text = "def format_money(amount):\n    return str((amount))\n\n\ndef unused_helper():\n    \
                return 1\n";
    io::write_file(&edited, text.as_bytes())
        .unwrap_or_else(|error| panic!("{}: {error}", edited.display()));
    assert_eq!(held.kind(&scanned, "render"), CardKind::Symbol);
    assert_eq!(held.parts(), [1; 8]);
}

#[test]
fn a_scan_that_arrives_while_the_prebuild_runs_waits_for_it() {
    let scanned = Scanned::new(&LINKED);
    let held = Held::new();
    assert_eq!(held.kind(&scanned, "pkg/report.py"), CardKind::File);
    let edited = scanned.reading().repo.root.join("pkg/money.py");
    let text = "def format_money(amount):\n    return amount\n\n\ndef added():\n    return 2\n";
    io::write_file(&edited, text.as_bytes())
        .unwrap_or_else(|error| panic!("{}: {error}", edited.display()));
    let graphs = &held.graphs;
    std::thread::scope(|scope| {
        let call = scope.spawn(|| kind(graphs, &scanned, "pkg/report.py"));
        held.waiting
            .recv()
            .expect("the new scan waits for the running prebuild");
        held.prebuilt();
        assert_eq!(call.join().ok(), Some(CardKind::File));
    });
    held.prebuilt();
    assert_eq!(held.graphs.builds.waits.load(Ordering::Relaxed), 1);
    assert_eq!(held.kind(&scanned, "added"), CardKind::Symbol);
}

// Twenty modules, each calling the one before it, so that one file holds a twentieth of the sites.
fn chained() -> Vec<(String, String)> {
    let module = |at: usize| {
        let call = if at == 0 {
            "    return 1\n".to_owned()
        } else {
            format!("    return step{}() + 1\n", at - 1)
        };
        let import = if at == 0 {
            String::new()
        } else {
            format!("from m{:02} import step{}\n\n\n", at - 1, at - 1)
        };
        (
            format!("m{at:02}.py"),
            format!("{import}def step{at}():\n{call}"),
        )
    };
    (0..20).map(module).collect()
}

fn chained_repo() -> Scanned {
    let files = chained();
    let pairs: Vec<(&str, &str)> = (files.iter())
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    Scanned::new(&pairs)
}

// The text of the trace of `target` over `graphs`.
fn trace_text(graphs: &KeptGraphs, scanned: &Scanned, target: &str) -> String {
    let request = TraceRequest {
        targets: vec![target.to_owned()],
        more: None,
    };
    let traced = trace_kept(&scanned.services, &scanned.reading(), &request, graphs);
    format!("{:?}", traced.value.map(|trace| trace.cards))
}

fn edit(scanned: &Scanned, path: &str, text: &str) {
    let edited = scanned.reading().repo.root.join(path);
    io::write_file(&edited, text.as_bytes())
        .unwrap_or_else(|error| panic!("{}: {error}", edited.display()));
}

// How many references and bases of a file's text the resolver tries to bind.
fn resolvable(path: &str, text: &str) -> usize {
    let language = syntax::Language::from_path(path).expect("a known language");
    let parsed = syntax::parse(language, Arc::from(text)).expect("the text parses");
    let facts = facts::extract(&parsed);
    let declared = facts::Declared::of(&facts.symbols, &facts.refs);
    let refs = facts.refs.iter();
    let sites = refs.filter(|reference| reference.is_resolvable() && !declared.declares(reference));
    let bases = facts.symbols.iter().flat_map(|symbol| &symbol.bases);
    let bases = bases.filter(|base| !facts::base_name(base).is_empty());
    sites.count() + bases.count()
}

fn pinned_graphs(bounds: Updates) -> KeptGraphs {
    let clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
    KeptGraphs::bounded(clock, bounds)
}

#[test]
fn a_body_edit_of_one_file_resolves_that_file_again_and_builds_nothing() {
    let scanned = chained_repo();
    let graphs = pinned_graphs(UPDATED);
    let before = trace_text(&graphs, &scanned, "step10");
    let text = "from m10 import step10\n\n\ndef step11():\n    value = step10()\n    return value + step10()\n";
    edit(&scanned, "m11.py", text);
    let after = trace_text(&graphs, &scanned, "step10");
    assert_ne!(after, before, "the edit adds a site of step10");
    let fresh = pinned_graphs(UPDATED);
    assert_eq!(after, trace_text(&fresh, &scanned, "step10"));
    let builds = graphs.builds();
    let built = (
        builds.indexes,
        builds.scopes,
        builds.graphs,
        builds.rebuilds,
    );
    assert_eq!(built, (1, 1, 1, 0));
    let resolved = (builds.updates, builds.resolved_files, builds.resolved_sites);
    assert_eq!(resolved, (1, 1, resolvable("m11.py", text)));
}

#[test]
fn a_panic_in_an_update_reaches_the_call_as_a_build_panic_does_and_the_next_call_builds() {
    let scanned = chained_repo();
    let clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
    let graphs = KeptGraphs::panicking(clock, "the update broke");
    trace_text(&graphs, &scanned, "step10");
    let text = "from m10 import step10\n\n\ndef step11():\n    return step10() + step10()\n";
    edit(&scanned, "m11.py", text);

    let call = panic::catch_unwind(AssertUnwindSafe(|| trace_text(&graphs, &scanned, "step10")));

    let payload = call.expect_err("the update's panic reaches the call");
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"the update broke"));
    let after = trace_text(&graphs, &scanned, "step10");
    assert_eq!(
        after,
        trace_text(&pinned_graphs(UPDATED), &scanned, "step10")
    );
    let builds = graphs.builds();
    assert_eq!((builds.graphs, builds.updates, builds.rebuilds), (2, 0, 1));
}

#[test]
fn an_update_past_its_share_of_the_sites_is_a_full_build() {
    let scanned = chained_repo();
    let text = "from m10 import step10\n\n\ndef step11():\n    return step10() + step10()\n";
    let narrow = Updates {
        sites: 1,
        ..UPDATED
    };
    let graphs = pinned_graphs(narrow);
    trace_text(&graphs, &scanned, "step10");
    edit(&scanned, "m11.py", text);
    trace_text(&graphs, &scanned, "step10");
    let builds = graphs.builds();
    assert_eq!((builds.graphs, builds.updates, builds.rebuilds), (2, 0, 1));
}

// Edits m11.py once for each count of sites, and returns the builds after each call.
fn edited_in_turn(graphs: &KeptGraphs, scanned: &Scanned, times: usize) -> Vec<Builds> {
    trace_text(graphs, scanned, "step10");
    let edits = (1..=times).map(|count| {
        let calls = "    step10()\n".repeat(count);
        let text = format!("from m10 import step10\n\n\ndef step11():\n{calls}    return 1\n");
        edit(scanned, "m11.py", &text);
        trace_text(graphs, scanned, "step10");
        graphs.builds()
    });
    edits.collect()
}

#[test]
fn a_kept_state_holds_at_most_its_count_of_scans() {
    let scanned = chained_repo();
    let bounds = Updates {
        scans: 3,
        replaced: 100,
        ..UPDATED
    };
    let graphs = pinned_graphs(bounds);
    let builds = edited_in_turn(&graphs, &scanned, 3);
    let counts: Vec<(usize, usize)> = (builds.iter())
        .map(|builds| (builds.updates, builds.rebuilds))
        .collect();
    assert_eq!(counts, [(1, 0), (2, 0), (2, 1)]);
}

// Each file holds about a twentieth of the sites, so the second replaced m11.py passes a tenth.
#[test]
fn a_kept_state_holds_replaced_facts_of_at_most_its_share_of_the_sites() {
    let scanned = chained_repo();
    let graphs = pinned_graphs(Updates {
        replaced: 10,
        ..UPDATED
    });
    let builds = edited_in_turn(&graphs, &scanned, 3);
    let counts: Vec<(usize, usize)> = (builds.iter())
        .map(|builds| (builds.updates, builds.rebuilds))
        .collect();
    assert_eq!(counts, [(1, 0), (1, 1), (2, 1)]);
}

#[test]
fn a_scan_of_other_paths_replaces_no_facts_in_the_store() {
    let mut stored = super::Stored::of(&[]);
    let one = graph::FileFacts {
        path: "a.py".to_owned(),
        language: syntax::Language::Python,
        line_count: 1,
        facts: facts::Facts::default(),
    };
    let two = graph::FileFacts {
        path: "b.py".to_owned(),
        ..one.clone()
    };
    stored.add(&[&one], &[&one, &two]);
    stored.add(&[&two], &[&one]);
    assert_eq!(stored.replaced, 0);
}
