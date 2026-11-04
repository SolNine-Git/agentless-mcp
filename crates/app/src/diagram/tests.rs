use super::{DiagramRequest, DiagramView, diagram, distinct, safe_label};
use crate::IntArg;
use crate::endpoints::scanned::{LINKED, Scanned};
use crate::testing::MANIFEST_DIR;

const LABELS: &str = "read_graph/labels";

fn drawn(focus: Option<&str>, grouped: bool, max_nodes: usize) -> DiagramView {
    let scanned = Scanned::new(&LINKED);
    let request = DiagramRequest {
        focus: focus.map(str::to_owned),
        max_nodes: IntArg::from_usize(max_nodes),
        max_edges: IntArg::from_usize(40),
        group_by_communities: grouped,
        resolution: None,
    };
    let outcome = diagram(&scanned.services, &scanned.reading(), &request);
    outcome.value.unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn a_label_keeps_only_the_allowlist_as_the_reference_does() {
    let cases: Vec<(String, String)> = io::testing::cases(MANIFEST_DIR, LABELS, "labels");
    assert!(cases.len() > 10);
    for (text, expected) in cases {
        assert_eq!(safe_label(&text), expected, "{text:?}");
    }
}

#[test]
fn a_label_keeps_the_underscores_of_the_path_and_merges_only_replaced_runs() {
    assert_eq!(
        safe_label("django/conf/__init__.py"),
        "django/conf/__init__.py"
    );
    assert_eq!(safe_label("a__\"];b"), "a___b");
}

#[test]
fn a_repeated_label_takes_the_reference_ordinal() {
    let cases: Vec<(Vec<String>, Vec<String>)> =
        io::testing::cases(MANIFEST_DIR, LABELS, "distinct");
    assert!(!cases.is_empty());
    for (labels, expected) in cases {
        assert_eq!(distinct(labels.into_iter()), expected);
    }
}

#[test]
fn a_focus_that_names_no_module_is_a_message_and_no_chart() {
    let view = drawn(Some(" zz_nothing "), false, 40);
    assert!(view.chart.is_none());
    assert_eq!(view.message, "no module matches zz_nothing");
    assert_eq!(view.focus, " zz_nothing ");
}

#[test]
fn the_node_bound_elides_the_lowest_ranked_modules() {
    let view = drawn(None, false, 2);
    let chart = view.chart.unwrap_or_else(|| panic!("a chart"));
    assert_eq!((view.nodes, view.elided), (2, 2));
    assert_eq!(chart.nodes.len(), 2);
    assert!(chart.groups.is_empty());
}

#[test]
fn the_focus_module_is_always_drawn_and_named() {
    let view = drawn(Some("main"), true, 1);
    let chart = view.chart.unwrap_or_else(|| panic!("a chart"));
    assert_eq!(view.focus, "pkg/main.py");
    let labels: Vec<&str> = (chart.groups.iter())
        .flat_map(|group| group.members.iter())
        .chain(chart.nodes.iter())
        .map(|node| node.label.as_str())
        .collect();
    assert_eq!(labels, ["pkg/main.py"]);
    assert!(view.grouped);
}
