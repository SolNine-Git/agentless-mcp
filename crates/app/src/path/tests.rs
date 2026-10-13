use graph::PathPolicy;

use super::{PathRequest, PathTrace, path};
use crate::endpoints::scanned::{LINKED, Scanned};
use crate::{Error, IntArg};

fn traced(source: &str, target: &str, max_visited: &str) -> Result<PathTrace, Error> {
    let scanned = Scanned::new(&LINKED);
    let request = PathRequest {
        source: source.to_owned(),
        target: target.to_owned(),
        policy: PathPolicy::default(),
        max_visited: IntArg::parse(max_visited).unwrap_or_else(|| panic!("{max_visited:?}")),
    };
    path(&scanned.services, &scanned.reading(), &request).value
}

#[test]
fn a_resolved_chain_reads_each_hop_in_the_direction_it_runs() {
    let trace = traced("main", "format_money", "20000").unwrap_or_else(|error| panic!("{error}"));
    assert!(trace.found && trace.endpoints_resolved && trace.message.is_empty());
    let steps: Vec<(&str, &str, &str)> = (trace.hops.iter())
        .map(|hop| (hop.arrow, hop.verb, hop.node.as_str()))
        .collect();
    assert_eq!(
        steps,
        [
            ("->", "references", "py:pkg/report.py::render"),
            ("->", "references", "py:pkg/money.py::format_money"),
        ]
    );
    assert_eq!(trace.source, "py:pkg/main.py::main");
}

#[test]
fn an_unresolved_endpoint_keeps_the_texts_and_joins_the_reasons() {
    let trace = traced("nope", "zilch", "1").unwrap_or_else(|error| panic!("{error}"));
    assert!(!trace.endpoints_resolved && !trace.found);
    assert_eq!(
        (trace.source.as_str(), trace.target.as_str()),
        ("nope", "zilch")
    );
    assert_eq!(
        trace.message,
        "no symbol or file matches nope; no symbol or file matches zilch"
    );
}

#[test]
fn no_path_names_the_tiers_the_search_left_out() {
    let trace = traced("unused_helper", "main", "20000").unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        trace.message,
        "no path from py:pkg/money.py::unused_helper to py:pkg/main.py::main over resolved \
         edges; unique and name-only-ambiguous edges were excluded"
    );
}

#[test]
fn the_node_bound_is_checked_before_the_scan() {
    let Err(Error::Kind(kernel::Error::OperationFailed(message))) = traced("a", "b", "0") else {
        panic!("a bound refusal");
    };
    assert_eq!(message, "max_visited must be at least 1, got 0");
}
