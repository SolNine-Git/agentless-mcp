use graph::PathPolicy;

use super::{PathRequest, PathTrace, path};
use crate::endpoints::scanned::{LINKED, Scanned};
use crate::{Error, IntArg};

fn traced(source: &str, target: &str, max_visited: &str) -> Result<PathTrace, Error> {
    traced_over(&LINKED, source, target, max_visited)
}

fn traced_over(
    files: &[(&str, &str)],
    source: &str,
    target: &str,
    max_visited: &str,
) -> Result<PathTrace, Error> {
    let scanned = Scanned::new(files);
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
fn a_data_key_has_no_edges_and_a_key_the_file_does_not_hold_has_the_expand_reason() {
    let mut files = LINKED.to_vec();
    files.push(("data.json", "{\n  \"receipt_header\": \"x\"\n}\n"));
    let keyed = |source: &str, target: &str| {
        traced_over(&files, source, target, "20000").unwrap_or_else(|error| panic!("{error}"))
    };
    let trace = keyed("json:data.json::receipt_header", "main");
    assert!(!trace.endpoints_resolved && !trace.found);
    assert_eq!(
        trace.message,
        "json:data.json::receipt_header is a data key, and a data key has no edges: Grep finds \
         the spellings of receipt_header."
    );
    let trace = keyed("main", "json:data.json::format_money");
    assert!(!trace.endpoints_resolved && !trace.found);
    assert_eq!(
        trace.message,
        "data.json keeps an outline of its keys, and format_money is not in it: the outline stops \
         at depth 2, at list items and at 500 keys. Read the lines with read slice, or Grep for \
         the key."
    );
    assert_eq!(
        (trace.source_label.as_str(), trace.target_label.as_str()),
        ("main", "json:data.json::format_money")
    );
    let trace = keyed("nope", "json:data.json::receipt_header");
    assert_eq!(
        trace.message,
        "no symbol or file matches nope; json:data.json::receipt_header is a data key, and a data \
         key has no edges: Grep finds the spellings of receipt_header."
    );
    let trace = keyed("json:data.json::receipt_header", "nope");
    assert_eq!(
        trace.message,
        "json:data.json::receipt_header is a data key, and a data key has no edges: Grep finds \
         the spellings of receipt_header; no symbol or file matches nope"
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
