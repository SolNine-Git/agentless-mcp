use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use kernel::testing::PinnedClock;

use super::{Gate, KeptGraphs};
use crate::endpoints::scanned::{LINKED, Scanned};
use crate::scan::KeptFacts;
use crate::symbol_graph::retention::RETAINED;
use crate::trace::{CardKind, Outcome, TraceRequest, trace_kept};

fn pinned() -> (Arc<PinnedClock>, KeptGraphs) {
    let clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
    let graphs = KeptGraphs::new(Arc::clone(&clock) as _);
    (clock, graphs)
}

fn roots(names: &[String]) -> Vec<PathBuf> {
    names.iter().map(PathBuf::from).collect()
}

#[test]
fn a_server_keeps_the_state_of_the_roots_it_called_last() {
    let (_, graphs) = pinned();
    let names: Vec<String> = (0..=RETAINED.roots).map(|at| format!("r{at}")).collect();
    let facts: Vec<Arc<KeptFacts>> = names.iter().map(|_| Arc::default()).collect();
    for (name, facts) in names.iter().zip(&facts) {
        graphs.keep(Path::new(name), Arc::clone(facts));
    }
    assert_eq!(graphs.kept_roots(), roots(&names[1..]));
    let oldest = &names[1];
    assert!(Arc::ptr_eq(&graphs.facts(Path::new(oldest)), &facts[1]));

    graphs.keep(Path::new(&names[0]), Arc::clone(&facts[0]));

    let mut kept = vec![names[0].clone(), oldest.clone()];
    kept.extend(names[3..].iter().cloned());
    assert_eq!(graphs.kept_roots(), roots(&kept));
    assert!(Arc::ptr_eq(&graphs.facts(Path::new(oldest)), &facts[1]));
}

#[test]
fn a_server_drops_the_state_of_a_root_after_the_idle_time_with_no_call() {
    let (clock, graphs) = pinned();
    let (early, late) = (Path::new("early"), Path::new("late"));
    let facts: Arc<KeptFacts> = Arc::default();
    graphs.keep(early, Arc::clone(&facts));
    let short = RETAINED.idle.checked_sub(Duration::from_secs(1));
    clock.advance(short.expect("the idle time is over a second"));
    graphs.keep(late, Arc::default());
    graphs.kept.sweep();
    assert_eq!(graphs.kept_roots(), [early, late]);
    assert!(Arc::ptr_eq(&graphs.facts(early), &facts));

    clock.advance(RETAINED.idle);
    graphs.kept.sweep();
    assert!(graphs.kept_roots().is_empty());

    graphs.keep(early, Arc::clone(&facts));
    clock.advance(RETAINED.idle);
    let after = graphs.facts(early);
    assert!(
        !Arc::ptr_eq(&after, &facts),
        "a call after the idle time reads no kept facts"
    );
    assert_eq!(graphs.kept_roots(), [early]);
}

// A server's state whose prebuilds wait for `open`, and send on `done` when they finish.
struct Held {
    graphs: KeptGraphs,
    open: Sender<()>,
    done: Receiver<()>,
}

impl Held {
    fn new() -> Held {
        let (open, opened) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let gate = Gate {
            open: Mutex::new(opened),
            done: Mutex::new(finished),
        };
        let clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
        let graphs = KeptGraphs::gated(clock, gate);
        Held { graphs, open, done }
    }

    fn kind(&self, scanned: &Scanned, target: &str) -> CardKind {
        let request = TraceRequest {
            targets: vec![target.to_owned()],
            more: None,
        };
        let traced = trace_kept(
            &scanned.services,
            &scanned.reading(),
            &request,
            &self.graphs,
        );
        let mut trace = traced.value.unwrap_or_else(|error| panic!("{error}"));
        match trace.cards.remove(0).outcome {
            Outcome::Card(card) => card.kind(),
            other => panic!("{target}: a card, got {other:?}"),
        }
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
