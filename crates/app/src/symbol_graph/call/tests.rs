use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use kernel::testing::PinnedClock;

use super::KeptGraphs;
use crate::scan::KeptFacts;
use crate::symbol_graph::retention::RETAINED;

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
