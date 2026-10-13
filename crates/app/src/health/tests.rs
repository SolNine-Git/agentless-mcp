use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::{HealthReport, HealthRequest, health};
use crate::IntArg;
use crate::clock::PinnedClock;
use crate::endpoints::scanned::{LINKED, Scanned};

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
