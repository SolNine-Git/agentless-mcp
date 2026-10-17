use std::sync::Arc;
use std::time::{Duration, SystemTime};

use kernel::{Deadline, Stopped};

use super::{HealthReport, HealthRequest, complexity, health};
use crate::IntArg;
use crate::clock::{PinnedClock, assert_spent, spent};
use crate::endpoints::scanned::{LINKED, Scanned};
use crate::scan::RepoFacts;

fn checked(scanned: &Scanned) -> HealthReport {
    let request = HealthRequest {
        limit: IntArg::from_usize(20),
    };
    let outcome = health(&scanned.services, &scanned.reading(), &request);
    outcome.value.unwrap_or_else(|error| panic!("{error}"))
}

fn labels(rows: &[super::HealthSymbol]) -> Vec<&str> {
    rows.iter().map(|row| row.label.as_str()).collect()
}

#[test]
fn the_graph_sections_count_only_binding_edges() {
    let report = checked(&Scanned::new(&LINKED));
    assert_eq!(report.symbols, 4);
    assert_eq!(labels(report.orphans.rows()), ["unused_helper"]);
    assert_eq!(
        labels(report.unused_exports.rows()),
        ["main", "unused_helper"]
    );
    assert_eq!(
        labels(report.hubs.rows()),
        ["render", "main", "format_money"]
    );
}

#[test]
fn every_file_in_scope_is_walked_and_measured() {
    let report = checked(&Scanned::new(&LINKED));
    let summary = &report.complexity;
    assert_eq!(
        (summary.files, summary.walked, summary.measured()),
        (4, 4, 4)
    );
    assert_eq!(summary.units, 4);
    assert!(!summary.stopped_by_budget());
    assert_eq!(report.hotspots.total(), 4);
    assert!(
        report
            .hotspots
            .rows()
            .iter()
            .all(|row| row.stable_id.is_some())
    );
}

#[test]
fn a_spent_budget_ends_the_walk_before_the_first_file() {
    let mut scanned = Scanned::new(&LINKED);
    scanned.services.clock = Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH));
    scanned.services.walk_budget = Duration::ZERO;
    let summary = checked(&scanned).complexity;
    assert_eq!((summary.files, summary.walked, summary.units), (4, 0, 0));
    assert!(summary.stopped_by_budget());
    assert!(summary.erosion.abs() < f64::EPSILON);
}

fn loaded(scanned: &Scanned) -> RepoFacts {
    let loaded = RepoFacts::load(&scanned.services, &scanned.reading());
    loaded.value.unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn a_spent_deadline_stops_the_health_call() {
    let scanned = Scanned::new(&LINKED);
    let deadline = spent();
    let request = HealthRequest {
        limit: IntArg::from_usize(20),
    };

    let outcome = health(
        &scanned.services,
        &scanned.reading().with_deadline(&deadline),
        &request,
    );

    assert_spent(&outcome.value.expect_err("the deadline stops the call"));
}

// The walk budget is half the remaining time, so a spent call measures no file and the walk
// does not stop it.
#[test]
fn a_spent_deadline_leaves_the_complexity_walk_no_budget() {
    let scanned = Scanned::new(&LINKED);
    let facts = loaded(&scanned);
    let deadline = spent();
    let reading = scanned.reading().with_deadline(&deadline);

    let walk = complexity::walk(&scanned.services, &reading, &facts);

    let walk = walk
        .value
        .expect("a walk with no budget is cut, not stopped");
    let summary = complexity::summary(&walk, 10, 20);
    assert_eq!((summary.files, summary.walked), (4, 0));
}

#[test]
fn a_cancelled_call_stops_the_complexity_walk() {
    let scanned = Scanned::new(&LINKED);
    let facts = loaded(&scanned);
    let deadline = Deadline::new(60.0, 0.0, Arc::new(PinnedClock::at(SystemTime::UNIX_EPOCH)));
    deadline.cancel();
    let reading = scanned.reading().with_deadline(&deadline);

    let walk = complexity::walk(&scanned.services, &reading, &facts);

    assert!(
        matches!(walk.value, Err(Stopped::Cancelled)),
        "{:?}",
        walk.value
    );
}
