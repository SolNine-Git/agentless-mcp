use super::{ChartEdge, DiagramRequest, DiagramView, Pair, bounded, diagram, distinct, safe_label};
use crate::IntArg;
use crate::endpoints::scanned::{LINKED, Scanned};
use crate::testing::MANIFEST_DIR;

const LABELS: &str = "read_graph/labels";

// d.py imports a.py; b.py names helper and twin, and c.py names twin, which a.py and b.py define.
const WEIGHED: [(&str, &str); 4] = [
    (
        "a.py",
        "def helper():\n    return 1\n\n\ndef twin():\n    return 2\n",
    ),
    (
        "b.py",
        "def twin():\n    return 3\n\n\ndef caller():\n    return helper() + twin()\n",
    ),
    ("c.py", "def use():\n    return twin()\n"),
    (
        "d.py",
        "from a import helper\n\n\ndef again():\n    return helper()\n",
    ),
];

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

fn pair(from: usize, to: usize, weight: f64, declared: bool) -> Pair {
    Pair {
        from,
        to,
        weight,
        declared,
    }
}

fn arrows(edges: &[ChartEdge]) -> Vec<(&str, &str, bool)> {
    (edges.iter())
        .map(|edge| (edge.from.as_str(), edge.to.as_str(), edge.solid))
        .collect()
}

#[test]
fn the_edge_bound_never_drops_a_declared_import() {
    let pairs = vec![
        pair(0, 1, 3.0, true),
        pair(1, 2, 3.0, true),
        pair(2, 3, 3.0, true),
        pair(3, 0, 5.0, false),
    ];
    let (edges, not_drawn) = bounded(pairs, 2);
    assert_eq!(
        arrows(&edges),
        [("n0", "n1", true), ("n1", "n2", true), ("n2", "n3", true)]
    );
    assert_eq!(not_drawn, 1);
}

#[test]
fn the_heaviest_reference_edges_fill_the_room_in_weight_order() {
    let pairs = || {
        vec![
            pair(0, 1, 3.0, true),
            pair(0, 2, 0.25, false),
            pair(1, 2, 1.5, false),
            pair(2, 0, 0.75, false),
            pair(3, 1, 2.0, false),
        ]
    };
    let (edges, not_drawn) = bounded(pairs(), 3);
    assert_eq!(
        arrows(&edges),
        [("n0", "n1", true), ("n1", "n2", false), ("n3", "n1", false)]
    );
    assert_eq!(not_drawn, 2);
    let (edges, not_drawn) = bounded(pairs(), 4);
    assert_eq!(
        arrows(&edges),
        [
            ("n0", "n1", true),
            ("n1", "n2", false),
            ("n2", "n0", false),
            ("n3", "n1", false)
        ]
    );
    assert_eq!(not_drawn, 1);
}

#[test]
fn reference_edges_of_equal_weight_go_in_id_order() {
    let pairs = || {
        vec![
            pair(2, 1, 1.0, false),
            pair(0, 3, 1.0, false),
            pair(1, 0, 1.0, false),
            pair(0, 2, 1.0, false),
        ]
    };
    let mut reversed = pairs();
    reversed.reverse();
    for given in [pairs(), reversed] {
        let (edges, not_drawn) = bounded(given, 2);
        assert_eq!(arrows(&edges), [("n0", "n2", false), ("n0", "n3", false)]);
        assert_eq!(not_drawn, 2);
    }
}

#[test]
fn the_count_is_the_reference_edges_not_drawn_at_every_bound() {
    let pairs = || {
        vec![
            pair(0, 1, 3.0, true),
            pair(1, 0, 2.0, false),
            pair(2, 0, 1.0, false),
            pair(2, 1, 0.5, false),
        ]
    };
    for max_edges in 0..=5 {
        let (edges, not_drawn) = bounded(pairs(), max_edges);
        let dashed = edges.iter().filter(|edge| !edge.solid).count();
        assert_eq!(edges.len() - dashed, 1, "bound {max_edges}");
        assert_eq!(
            dashed,
            max_edges.saturating_sub(1).min(3),
            "bound {max_edges}"
        );
        assert_eq!(dashed + not_drawn, 3, "bound {max_edges}");
    }
}

#[test]
fn a_diagram_under_the_edge_bound_draws_every_edge() {
    let pairs = || {
        vec![
            pair(0, 1, 3.0, true),
            pair(1, 2, 0.5, false),
            pair(2, 0, 1.0, false),
            pair(2, 1, 3.0, true),
        ]
    };
    let every = [
        ("n0", "n1", true),
        ("n1", "n2", false),
        ("n2", "n0", false),
        ("n2", "n1", true),
    ];
    for max_edges in [4, 40] {
        let (edges, not_drawn) = bounded(pairs(), max_edges);
        assert_eq!(arrows(&edges), every, "bound {max_edges}");
        assert_eq!(not_drawn, 0, "bound {max_edges}");
    }
}

#[test]
fn a_diagram_past_the_edge_bound_draws_its_import_and_its_heaviest_reference() {
    let scanned = Scanned::new(&WEIGHED);
    let request = DiagramRequest {
        focus: None,
        max_nodes: IntArg::from_usize(40),
        max_edges: IntArg::from_usize(2),
        group_by_communities: false,
        resolution: None,
    };
    let outcome = diagram(&scanned.services, &scanned.reading(), &request);
    let view = outcome.value.unwrap_or_else(|error| panic!("{error}"));
    let chart = view.chart.unwrap_or_else(|| panic!("a chart"));
    assert_eq!(
        arrows(&chart.edges),
        [("n1", "n0", false), ("n3", "n0", true)]
    );
    assert_eq!(view.edges_over_bound, 2);
}
