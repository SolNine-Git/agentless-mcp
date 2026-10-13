use super::map_rows_text;
use crate::RationaleNode;
use crate::map::{MapEntry, MapFile};

fn file(path: &str, rank: f64, churn: (Option<usize>, Option<i64>)) -> MapFile {
    MapFile {
        path: path.to_owned(),
        rank,
        entries: Vec::new(),
        total: 0,
        id_prefix: "",
        reached: true,
        commits_window: churn.0,
        last_commit_days: churn.1,
    }
}

fn entry(line: u32, signature: &str, stable_id: &str, depth: usize, name: &str) -> MapEntry {
    MapEntry {
        line,
        signature: signature.to_owned(),
        stable_id: stable_id.to_owned(),
        depth,
        rationales: Vec::new(),
        name: name.to_owned(),
    }
}

// The expected texts are Python render_map's over the same rows.
#[test]
fn rows_carry_the_pattern_line_rationales_and_both_omission_lines() {
    let mut first = entry(2, "def f(x)", "py:a.py::f", 0, "f");
    first.rationales.push(RationaleNode {
        stable_id: "py:a.py::f#why@3".to_owned(),
        parent_id: "py:a.py::f".to_owned(),
        line: 3,
        kind: "why",
        text: "keeps\nit".to_owned(),
        citations: Vec::new(),
    });
    let mut ranked = file("a.py", 0.123_456, (Some(3), Some(4)));
    ranked.entries = vec![first, entry(9, "def m(self)", "py:a.py::C.m", 1, "")];
    ranked.total = 5;
    ranked.id_prefix = "py";
    let mut unreached = file("b.py", 0.000_05, (Some(0), None));
    unreached.total = 2;
    unreached.reached = false;
    let expected = "a.py  (rank 0.1235, 3c/90d, last 4d)\n  stable ids: py:a.py::<QualifiedName>\n  \
                    def f(x)  [f] @2\n      # WHY: keeps\\nit  [py:a.py::f#why@3 -> py:a.py::f] @3\n      \
                    def m(self)  [py:a.py::C.m] @9\n... 3 more symbols in this file not listed\n\n\
                    b.py  (rank 0.0001, 0c/90d)\n... 2 symbols in this file; the focus has no \
                    reference path to it, so no budget shows them\n";
    assert_eq!(map_rows_text(&[ranked, unreached]), expected);
}

#[test]
fn one_churn_pair_on_every_file_drops_the_clause_but_a_lone_file_keeps_it() {
    let same = [
        file("a.py", 0.5, (Some(1), Some(2))),
        file("b.py", 0.25, (Some(1), Some(2))),
    ];
    assert_eq!(
        map_rows_text(&same),
        "a.py  (rank 0.5000)\n\nb.py  (rank 0.2500)\n"
    );
    let lone = "a.py  (rank 0.5000, 1c/90d, last 2d)\n";
    assert_eq!(map_rows_text(&same[..1]), lone);
    assert_eq!(map_rows_text(&[]), "no ranked files\n");
}
