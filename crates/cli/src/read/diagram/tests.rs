use super::{drift_summary, shown_path};

#[test]
fn a_shown_path_drops_empty_and_dot_segments() {
    let given = [
        "docs/graph.md",
        "./docs//graph.md/",
        "",
        ".",
        "//net/x",
        "///x",
        "a/../b",
    ];
    let shown = given.map(shown_path);
    let expected = [
        "docs/graph.md",
        "docs/graph.md",
        ".",
        ".",
        "//net/x",
        "/x",
        "a/../b",
    ];
    assert_eq!(shown, expected);
}

#[test]
fn a_drift_names_the_first_differing_line_or_the_longer_side() {
    assert_eq!(
        drift_summary("a\nb\n", "a\nc\n"),
        [
            "committed 3 lines, regenerated 3 lines",
            "first difference at line 2:",
            "  committed:   b",
            "  regenerated: c",
        ]
    );
    assert_eq!(
        drift_summary("a", "a\n"),
        [
            "committed 1 lines, regenerated 2 lines",
            "identical for the first 1 lines, then one is longer",
        ]
    );
}
